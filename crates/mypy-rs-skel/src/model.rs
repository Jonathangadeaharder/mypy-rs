//! The skeleton's own class model: the semantic facts the checker builds
//! from the AST (type variables, bases, MRO, members) plus the bridge
//! that turns a model into a kernel `TypeInfoSnapshot`. The skeleton owns
//! the corpus classes; committed fixtures carry the stdlib facts.

use std::collections::{BTreeMap, HashMap, HashSet};

use type_kernel::skeleton_api::{encode_type, Type, TypeInfoSnapshot};

/// The MRO entries mypy gives the two classes the skeleton hardcodes
/// facts for: `typing.Generic` and `builtins.object`.
pub const GENERIC_MRO: &[&str] = &["typing.Generic", "builtins.object"];
pub const OBJECT_MRO: &[&str] = &["builtins.object"];

/// A callable shape the checker tracks for methods and module
/// functions. Parameters are (name, type); self never appears.
#[derive(Debug, Clone)]
pub struct Sig {
    pub params: Vec<(String, Type)>,
    pub ret: Type,
}

/// One member of a class model.
#[derive(Debug, Clone)]
pub enum Member {
    /// A class-level annotated attribute (`kind: str = "shape"`).
    ClassVar(Type),
    /// An implicit instance attribute (`self.x = ...`).
    InstanceVar(Type),
    Method(Sig),
}

/// A type variable bound in a class frame: the raw id is the 1-based
/// position in the class's type-variable list, mirroring mypy's
/// `TypeVarLikeScope.class_frame` numbering (the counter lives on the
/// popped child scope, so every top-level class restarts at 1). The
/// kernel's `expand_type_by_instance` keys class-frame tvars by
/// position, so any other numbering defers every generic subtype check.
#[derive(Debug, Clone)]
pub struct TvarInfo {
    /// The string passed to `TypeVar`: the kernel-facing name.
    pub name: String,
    /// The module-level name bound to the TypeVar: the name annotations
    /// resolve through, as in mypy's `TypeVarLikeScope.class_frame`.
    pub binding: String,
    pub fullname: String,
    pub raw_id: i64,
}

/// The skeleton-side facts of one corpus class.
#[derive(Debug)]
pub struct ClassModel {
    pub fullname: String,
    pub tvars: Vec<TvarInfo>,
    /// (base fullname, args as stored by mypy: the subclass frame's
    /// copies of the type variables). The implicit `builtins.object`
    /// base is included.
    pub bases: Vec<(String, Vec<Type>)>,
    pub mro: Vec<String>,
    pub members: BTreeMap<String, Member>,
}

/// A bare `mypy.types.Instance` of `fullname`.
pub fn instance(fullname: &str, args: Vec<Type>) -> Type {
    Type::Instance {
        type_ref: fullname.to_string(),
        args,
        last_known_value: None,
        extra_attrs: None,
    }
}

pub fn object_type() -> Type {
    instance("builtins.object", Vec::new())
}

/// The `TypeVarType` node for `tvar` in the namespace of `ns`, with the
/// defaults mypy gives an unrestricted TypeVar. The default is mypy's
/// "no default" sentinel: AnyType with type_of_any = 4.
pub fn tvar_type(tvar: &TvarInfo, namespace: &str) -> Type {
    Type::TypeVarType {
        name: tvar.name.clone(),
        fullname: tvar.fullname.clone(),
        raw_id: tvar.raw_id,
        namespace: namespace.to_string(),
        values: Vec::new(),
        upper_bound: Box::new(object_type()),
        default: Box::new(Type::AnyType {
            type_of_any: 4,
            source_any: None,
            missing_import_name: None,
        }),
        variance: 0,
        meta_level: 0,
    }
}

/// The class-frame copies of the model's type variables, in order.
pub fn class_frame_tvars(model: &ClassModel) -> Vec<Type> {
    model
        .tvars
        .iter()
        .map(|tvar| tvar_type(tvar, &model.fullname))
        .collect()
}

pub type SubstEnv = HashMap<(i64, String), Type>;

/// Build the env mapping the class-frame type variables of `model` to
/// `args`. A length mismatch is a broken invariant (the caller
/// validated the argument count), so it is a checked failure, never a
/// silently truncating zip.
pub fn frame_env(model: &ClassModel, args: &[Type]) -> Result<SubstEnv, String> {
    if model.tvars.len() != args.len() {
        return Err(format!(
            "the class frame of {} holds {} type variables but {} arguments",
            model.fullname,
            model.tvars.len(),
            args.len()
        ));
    }
    let mut env = SubstEnv::new();
    for (tvar, arg) in model.tvars.iter().zip(args) {
        env.insert((tvar.raw_id, model.fullname.clone()), arg.clone());
    }
    Ok(env)
}

/// Replace every type variable whose (raw id, namespace) is in `env`.
/// Only the variants the subset can build (TypeVarType, Instance,
/// TypeType) recur; every other variant passes through unchanged.
pub fn subst(t: &Type, env: &SubstEnv) -> Type {
    match t {
        Type::TypeVarType {
            raw_id, namespace, ..
        } => match env.get(&(*raw_id, namespace.clone())) {
            Some(replacement) => replacement.clone(),
            None => t.clone(),
        },
        Type::Instance {
            type_ref,
            args,
            last_known_value,
            extra_attrs,
        } => {
            let substituted = args.iter().map(|arg| subst(arg, env)).collect();
            let lkv = last_known_value.as_ref().map(|boxed| subst(boxed, env));
            Type::Instance {
                type_ref: type_ref.clone(),
                args: substituted,
                last_known_value: lkv.map(Box::new),
                extra_attrs: extra_attrs.clone(),
            }
        }
        Type::TypeType { item, is_type_form } => Type::TypeType {
            item: Box::new(subst(item, env)),
            is_type_form: *is_type_form,
        },
        other => other.clone(),
    }
}

pub fn subst_sig(sig: &Sig, env: &SubstEnv) -> Sig {
    Sig {
        params: sig
            .params
            .iter()
            .map(|(name, t)| (name.clone(), subst(t, env)))
            .collect(),
        ret: subst(&sig.ret, env),
    }
}

/// C3 linearization over the base fullname lists. `base_mros` holds the
/// computed MRO of every base, in class order. Fails on an inconsistent
/// hierarchy the corpus cannot produce.
pub fn linearize(class_fullname: &str, base_mros: &[Vec<String>]) -> Result<Vec<String>, String> {
    if base_mros.iter().any(|mro| mro.is_empty()) {
        return Err(format!(
            "a base of {class_fullname} has an empty MRO; the corpus cannot produce this"
        ));
    }
    let mut sequences: Vec<Vec<String>> = base_mros.to_vec();
    sequences.push(base_mros.iter().map(|mro| mro[0].clone()).collect());
    let mut mro = vec![class_fullname.to_string()];
    loop {
        sequences.retain(|seq| !seq.is_empty());
        if sequences.is_empty() {
            return Ok(mro);
        }
        let mut head: Option<String> = None;
        for seq in &sequences {
            let candidate = &seq[0];
            let in_tail = sequences
                .iter()
                .any(|other| other.iter().skip(1).any(|entry| entry == candidate));
            if !in_tail {
                head = Some(candidate.clone());
                break;
            }
        }
        let Some(candidate) = head else {
            return Err(format!(
                "class hierarchy under {class_fullname} is not linearizable"
            ));
        };
        mro.push(candidate.clone());
        for seq in &mut sequences {
            if seq.first() == Some(&candidate) {
                seq.remove(0);
            }
        }
    }
}

/// The kernel snapshot for `model`. Blob fields (`bases`, per-tvar upper
/// bounds) are encoded through the kernel's own wire writer so the
/// kernel reads exactly the format `expand_type_by_instance` expects.
pub fn snapshot(model: &ClassModel) -> Result<TypeInfoSnapshot, String> {
    let mut bases = Vec::with_capacity(model.bases.len());
    for (base_ref, args) in &model.bases {
        let base_type = instance(base_ref, args.clone());
        bases.push(encode_type(&base_type)?);
    }
    let mut type_var_upper_bounds = Vec::with_capacity(model.tvars.len());
    let mut type_vars_with_variance = Vec::with_capacity(model.tvars.len());
    let mut type_var_raw_ids = Vec::with_capacity(model.tvars.len());
    for tvar in &model.tvars {
        type_var_upper_bounds.push(encode_type(&object_type())?);
        type_vars_with_variance.push((tvar.name.clone(), 0, 0));
        type_var_raw_ids.push(tvar.raw_id);
    }
    let mut member_info = HashMap::new();
    let mut member_definers = HashMap::new();
    for (name, member) in &model.members {
        let (info_pair, definer_kind) = match member {
            Member::ClassVar(_) => ((false, true), 2),
            Member::InstanceVar(_) => ((true, false), 2),
            Member::Method(_) => ((false, false), 0),
        };
        member_info.insert(name.clone(), info_pair);
        member_definers.insert(name.clone(), (definer_kind, model.fullname.clone()));
    }
    Ok(TypeInfoSnapshot {
        fullname: model.fullname.clone(),
        name: model
            .fullname
            .rsplit('.')
            .next()
            .unwrap_or(&model.fullname)
            .to_string(),
        is_protocol: false,
        is_enum: false,
        enum_members: Vec::new(),
        fallback_to_any: false,
        meta_fallback_to_any: false,
        is_named_tuple: false,
        is_newtype: false,
        has_type_var_tuple_type: false,
        has_param_spec_type: false,
        is_abstract: false,
        type_vars: model.tvars.iter().map(|t| t.name.clone()).collect(),
        mro: model.mro.clone(),
        protocol_members: Vec::new(),
        has_base: model.mro.iter().cloned().collect::<HashSet<_>>(),
        promote_bytes: Vec::new(),
        alt_promote_fullname: None,
        metaclass_fullname: Some(String::new()),
        bases,
        tuple_type: None,
        type_var_tuple_prefix: None,
        type_var_tuple_suffix: None,
        type_var_tuple_fallback: None,
        type_vars_with_variance,
        type_var_upper_bounds,
        type_var_raw_ids,
        member_info,
        member_definers,
    })
}

/// Insert (or refresh) the model's snapshot in the resolver. Re-inserting
/// replaces the previous snapshot, which is how member registration
/// becomes visible to the kernel mid-class.
pub fn refresh_snapshot(
    model: &ClassModel,
    resolver: &mut type_kernel::skeleton_api::TypeResolver,
) -> Result<(), String> {
    let snap = snapshot(model)?;
    resolver.insert(model.fullname.clone(), snap);
    Ok(())
}
