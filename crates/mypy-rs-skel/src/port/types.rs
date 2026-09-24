//! Standalone driver glue: types.
//!
//! Build and manipulate skeleton-side type values through the kernel type
//! algebra.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::types`. It owns no checking logic of its
//! own: it adapts skeleton records (`crate::model`, `crate::fixtures`) to
//! the kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `types` lane for this wave. Add
//! `#[cfg(test)]` unit tests here.
//!
//! # What this adapter adds over the kernel surface
//!
//! Two things, both adaptations rather than logic:
//!
//! * One handle, [`Algebra`], holding the `SubtypeContext` and the
//!   `TypeResolver` a pass shares, so a call site passes the operands and
//!   nothing else. `crate::check::Driver` already owns both by value.
//! * A kernel decline becomes a [`Declined`] error naming the mypy
//!   function and the operand shapes. The skeleton has no Python to fall
//!   back to, so a decline is a loud rejection (wave-1 rule 6), never a
//!   value to invent. Every fallible method returns `Result`, never
//!   `Option`.
//!
//! Infallible shape predicates (`is_named_instance`, `is_tuple`) are not
//! wrapped or re-exported: `type_kernel::standalone::types` already
//! exposes them, and a pass-through here would add a call and hide
//! nothing. [`describe`] is this module's own, because the standalone
//! path has no mypy-format type printer.

use std::fmt;

use type_kernel::standalone::types::{
    is_overlapping_types, is_same_type, is_subtype, join_types, make_simplified_union,
    map_instance_to_supertype, meet_types, narrow_declared_type, trivial_join, trivial_meet,
    SubtypeContext, Type, TypeResolver,
};

/// A kernel decline: the type algebra could not reproduce mypy's answer
/// from the facts the resolver carries.
///
/// `operation` is the mypy function the kernel ports, so a decline reads
/// against the same source the oracle runs. `operands` describes the
/// shapes involved, through [`describe`], because no standalone pretty
/// printer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    pub operation: &'static str,
    pub operands: String,
}

impl fmt::Display for Declined {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} declined on {}", self.operation, self.operands)
    }
}

/// Name the shape of `t` for a decline message: the class fullname where
/// the variant carries one, a short family name otherwise.
///
/// `typeinfo::render_type` needs a Python interpreter, so the standalone
/// path has no mypy-format type printer; this is the honest description a
/// rejection can carry. The match is exhaustive with no catch-all arm, so
/// a new wire variant fails the build here instead of printing as an
/// opaque blob.
pub fn describe(t: &Type) -> String {
    match t {
        Type::Instance { type_ref, .. } => type_ref.clone(),
        Type::TypeAliasType { type_ref, .. } => format!("alias {type_ref}"),
        Type::TypeVarType { fullname, .. } => format!("type var {fullname}"),
        Type::ParamSpecType { fullname, .. } => format!("param spec {fullname}"),
        Type::TypeVarTupleType { fullname, .. } => format!("type var tuple {fullname}"),
        Type::UnboundType { name, .. } => format!("unbound {name}"),
        Type::UnpackType { .. } => "unpack".to_string(),
        Type::AnyType { .. } => "Any".to_string(),
        Type::UninhabitedType { .. } => "<nothing>".to_string(),
        Type::NoneType => "None".to_string(),
        Type::ErasedType => "erased".to_string(),
        Type::DeletedType { .. } => "deleted".to_string(),
        Type::CallableType { name, .. } => match name {
            Some(name) => format!("callable {name}"),
            None => "callable".to_string(),
        },
        Type::Overloaded { items } => format!("overloaded of {}", items.len()),
        Type::TupleType { items, .. } => format!("tuple of {}", items.len()),
        Type::TypedDictType { .. } => "typed dict".to_string(),
        Type::LiteralType { value, .. } => format!("literal {value:?}"),
        Type::UnionType { items, .. } => {
            let parts: Vec<String> = items.iter().map(describe).collect();
            format!("union of [{}]", parts.join(", "))
        }
        Type::TypeType { item, .. } => format!("type of {}", describe(item)),
        Type::Parameters(_) => "parameters".to_string(),
        Type::PartialType { .. } => "partial".to_string(),
    }
}

/// Join the operand descriptions of a decline message.
fn describe_all(operands: &[&Type]) -> String {
    let parts: Vec<String> = operands.iter().copied().map(describe).collect();
    parts.join(" and ")
}

/// A decline for a two-operand operation.
fn declined(operation: &'static str, left: &Type, right: &Type) -> Declined {
    Declined {
        operation,
        operands: describe_all(&[left, right]),
    }
}

/// A decline for an operation over a list of operands.
fn declined_list(operation: &'static str, items: &[Type]) -> Declined {
    let operands: Vec<&Type> = items.iter().collect();
    Declined {
        operation,
        operands: describe_all(&operands),
    }
}

/// The type algebra as the skeleton calls it: the `SubtypeContext` and
/// the `TypeResolver` one pass shares, so a call site passes operands and
/// nothing else.
///
/// Borrows both, because `crate::check::Driver` owns them for the whole
/// run and the wave-2 integration lane builds this handle from those two
/// fields. No method here decides anything the kernel does not: each is
/// one kernel call plus the decline-to-error mapping.
pub struct Algebra<'a> {
    ctx: &'a SubtypeContext,
    resolver: &'a TypeResolver,
}

impl<'a> Algebra<'a> {
    /// Borrow one pass's context and resolver.
    pub fn new(ctx: &'a SubtypeContext, resolver: &'a TypeResolver) -> Self {
        Self { ctx, resolver }
    }

    /// `mypy.join.join_types`: the least upper bound of two types.
    pub fn join(&self, s: &Type, t: &Type) -> Result<Type, Declined> {
        let answer = join_types(s, t, self.ctx, self.resolver);
        match answer {
            Some(joined) => Ok(joined),
            None => Err(declined("join_types", s, t)),
        }
    }

    /// `mypy.join.trivial_join`: the subtype-only join, the fallback mypy
    /// uses where a structural join is not wanted.
    pub fn trivial_join(&self, s: &Type, t: &Type) -> Result<Type, Declined> {
        let answer = trivial_join(s, t, self.ctx, self.resolver);
        match answer {
            Some(joined) => Ok(joined),
            None => Err(declined("trivial_join", s, t)),
        }
    }

    /// `mypy.meet.meet_types`: the greatest lower bound of two types.
    pub fn meet(&self, s: &Type, t: &Type) -> Result<Type, Declined> {
        let answer = meet_types(s, t, self.ctx, self.resolver);
        match answer {
            Some(met) => Ok(met),
            None => Err(declined("meet_types", s, t)),
        }
    }

    /// `mypy.meet.trivial_meet`: the subtype-only meet.
    pub fn trivial_meet(&self, s: &Type, t: &Type) -> Result<Type, Declined> {
        let answer = trivial_meet(s, t, self.ctx, self.resolver);
        match answer {
            Some(met) => Ok(met),
            None => Err(declined("trivial_meet", s, t)),
        }
    }

    /// `mypy.typeops.make_simplified_union` at mypy's default flags: the
    /// union every diagnostic-facing type is built with.
    pub fn union(&self, items: &[Type]) -> Result<Type, Declined> {
        let answer = make_simplified_union(items, self.ctx, self.resolver);
        match answer {
            Some(union) => Ok(union),
            None => Err(declined_list("make_simplified_union", items)),
        }
    }

    /// `mypy.meet.narrow_declared_type`: narrow a declared type by a
    /// narrower one. `strict_optional` comes from the pass context, which
    /// is where the kernel's separate seam argument lives here.
    pub fn narrow(&self, declared: &Type, narrowed: &Type) -> Result<Type, Declined> {
        let strict = self.ctx.strict_optional;
        let answer = narrow_declared_type(declared, narrowed, strict, self.resolver);
        match answer {
            Some(narrowed) => Ok(narrowed),
            None => Err(declined("narrow_declared_type", declared, narrowed)),
        }
    }

    /// `mypy.meet.is_overlapping_types`. `ignore_promotions` comes from
    /// the pass context; `overlap_for_overloads` stays a parameter because
    /// only overload resolution sets it, and that is the call lane's
    /// decision to make, not this adapter's.
    pub fn overlaps(
        &self,
        left: &Type,
        right: &Type,
        overlap_for_overloads: bool,
    ) -> Result<bool, Declined> {
        let ignore_promotions = self.ctx.ignore_promotions;
        let strict = self.ctx.strict_optional;
        let answer = is_overlapping_types(
            left,
            right,
            ignore_promotions,
            overlap_for_overloads,
            strict,
            self.resolver,
        );
        match answer {
            Some(overlaps) => Ok(overlaps),
            None => Err(declined("is_overlapping_types", left, right)),
        }
    }

    /// `mypy.subtypes.is_subtype`: whether every value of `left` is also
    /// a value of `right`. This is the same entry `crate::check` already
    /// calls through `skeleton_api`, reached here through the pass handle.
    pub fn subsumes(&self, left: &Type, right: &Type) -> Result<bool, Declined> {
        let answer = is_subtype(left, right, self.ctx, self.resolver);
        match answer {
            Some(subsumes) => Ok(subsumes),
            None => Err(declined("is_subtype", left, right)),
        }
    }

    /// `mypy.subtypes.is_same_type`. `ignore_promotions` comes from the
    /// pass context.
    pub fn same(&self, a: &Type, b: &Type) -> Result<bool, Declined> {
        let ignore_promotions = self.ctx.ignore_promotions;
        let strict = self.ctx.strict_optional;
        let answer = is_same_type(a, b, ignore_promotions, strict, self.resolver);
        match answer {
            Some(same) => Ok(same),
            None => Err(declined("is_same_type", a, b)),
        }
    }

    /// `mypy.maptype.map_instance_to_supertype`: the arguments
    /// `left_ref[left_args]` presents to one of its supertypes. The
    /// operands are class names, so the decline names them directly.
    pub fn supertype_args(
        &self,
        left_ref: &str,
        left_args: &[Type],
        right_ref: &str,
    ) -> Result<Vec<Type>, Declined> {
        let answer = map_instance_to_supertype(left_ref, left_args, right_ref, self.resolver);
        match answer {
            Some(args) => Ok(args),
            None => Err(Declined {
                operation: "map_instance_to_supertype",
                operands: format!("{left_ref} as a {right_ref}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use type_kernel::skeleton_api::{encode_type, TypeInfoSnapshot};
    use type_kernel::standalone::types::make_union;

    fn ctx() -> SubtypeContext {
        SubtypeContext {
            strict_optional: true,
            ..SubtypeContext::default()
        }
    }

    fn instance(type_ref: &str) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args: Vec::new(),
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn alias_ref(fullname: &str) -> Type {
        Type::TypeAliasType {
            args: Vec::new(),
            type_ref: fullname.to_string(),
            is_recursive: false,
        }
    }

    /// One class snapshot declaring `bases`, each base encoded through
    /// the kernel's own wire writer the way `crate::model::snapshot` does.
    fn snap(fullname: &str, bases: &[&str]) -> TypeInfoSnapshot {
        let mut blobs = Vec::new();
        let mut has_base = HashSet::from([fullname.to_string()]);
        let mut mro = vec![fullname.to_string()];
        for base in bases {
            let encoded = encode_type(&instance(base));
            blobs.push(encoded.expect("an instance must encode"));
            has_base.insert((*base).to_string());
            mro.push((*base).to_string());
        }
        has_base.insert("builtins.object".to_string());
        mro.push("builtins.object".to_string());
        TypeInfoSnapshot {
            fullname: fullname.to_string(),
            name: fullname.to_string(),
            mro,
            has_base,
            bases: blobs,
            ..Default::default()
        }
    }

    fn insert(r: &mut TypeResolver, fullname: &str, bases: &[&str]) {
        let snapshot = snap(fullname, bases);
        r.insert(fullname.to_string(), snapshot);
    }

    /// `a.A`, a subclass `a.B`, and an unrelated `a.Z`.
    fn hierarchy() -> TypeResolver {
        let mut r = TypeResolver::new();
        insert(&mut r, "a.A", &[]);
        insert(&mut r, "a.B", &["a.A"]);
        insert(&mut r, "a.Z", &[]);
        r
    }

    /// The two disjoint stdlib classes the narrowing tests need.
    fn builtins() -> TypeResolver {
        let mut r = TypeResolver::new();
        insert(&mut r, "builtins.int", &[]);
        insert(&mut r, "builtins.str", &[]);
        r
    }

    #[test]
    fn join_of_two_subclasses_is_their_common_base() {
        let mut r = TypeResolver::new();
        insert(&mut r, "a.C", &[]);
        insert(&mut r, "a.D", &["a.C"]);
        insert(&mut r, "a.E", &["a.C"]);
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let left = instance("a.D");
        let right = instance("a.E");
        assert_eq!(alg.join(&left, &right), Ok(instance("a.C")));
    }

    #[test]
    fn meet_of_a_class_and_its_subclass_is_the_subclass() {
        let r = hierarchy();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let base = instance("a.A");
        let derived = instance("a.B");
        assert_eq!(alg.meet(&base, &derived), Ok(derived));
    }

    #[test]
    fn union_drops_a_member_another_covers() {
        let r = hierarchy();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let items = [instance("a.A"), instance("a.B")];
        assert_eq!(alg.union(&items), Ok(instance("a.A")));
    }

    #[test]
    fn narrow_keeps_the_overlapping_item_of_a_union() {
        let r = builtins();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let int = instance("builtins.int");
        let text = instance("builtins.str");
        let items = [int.clone(), text];
        let declared = alg.union(&items).unwrap();
        assert_eq!(alg.narrow(&declared, &int), Ok(int));
    }

    #[test]
    fn overlaps_answers_for_a_repeated_instance() {
        let r = builtins();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let int = instance("builtins.int");
        assert_eq!(alg.overlaps(&int, &int, false), Ok(true));
    }

    #[test]
    fn subsumes_same_and_supertype_args_follow_the_snapshots() {
        let r = hierarchy();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let base = instance("a.A");
        let derived = instance("a.B");
        assert_eq!(alg.subsumes(&derived, &base), Ok(true));
        assert_eq!(alg.same(&base, &base), Ok(true));
        assert_eq!(alg.same(&base, &derived), Ok(false));
        let args = [instance("builtins.int")];
        let mapped = alg.supertype_args("a.A", &args, "a.A");
        assert_eq!(mapped, Ok(args.to_vec()));
    }

    #[test]
    fn trivial_variants_give_the_subtype_answer() {
        let r = hierarchy();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let base = instance("a.A");
        let derived = instance("a.B");
        let other = instance("a.Z");
        assert_eq!(alg.trivial_join(&base, &derived), Ok(instance("a.A")));
        assert_eq!(alg.trivial_meet(&base, &derived), Ok(derived));
        let empty = Type::UninhabitedType { ambiguous: false };
        assert_eq!(alg.trivial_meet(&base, &other), Ok(empty));
    }

    #[test]
    fn a_decline_is_reported_not_guessed() {
        // The skeleton has no Python fallback, so an alias the resolver
        // cannot expand must surface as a named rejection.
        let r = hierarchy();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let alias = alias_ref("mod.A");
        let base = instance("a.A");
        let out = alg.join(&alias, &base);
        let Err(report) = &out else {
            panic!("an alias operand must decline, got {out:?}");
        };
        assert_eq!(report.operation, "join_types");
        assert!(report.operands.contains("mod.A"));
        let rendered = report.to_string();
        assert_eq!(rendered, "join_types declined on alias mod.A and a.A");
    }

    #[test]
    fn narrow_declines_when_the_resolver_carries_no_facts() {
        let r = TypeResolver::new();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let unknown = instance("a.Unknown");
        let out = alg.narrow(&unknown, &unknown);
        let Err(report) = &out else {
            panic!("an unknown class must decline, got {out:?}");
        };
        assert_eq!(report.operation, "narrow_declared_type");
    }

    #[test]
    fn supertype_args_declines_on_an_unknown_class() {
        let r = hierarchy();
        let context = ctx();
        let alg = Algebra::new(&context, &r);
        let out = alg.supertype_args("a.X", &[], "a.Y");
        let Err(report) = &out else {
            panic!("an unsnapshotted class must decline, got {out:?}");
        };
        assert_eq!(report.operation, "map_instance_to_supertype");
        assert_eq!(report.operands, "a.X as a a.Y");
    }

    #[test]
    fn describe_names_the_shape_of_a_type() {
        assert_eq!(describe(&instance("a.A")), "a.A");
        assert_eq!(describe(&alias_ref("mod.A")), "alias mod.A");
        let items = vec![instance("a.A"), instance("a.Z")];
        let union = make_union(items);
        assert_eq!(describe(&union), "union of [a.A, a.Z]");
    }

    #[test]
    fn a_decline_message_names_operation_and_operands() {
        let base = instance("a.A");
        let other = instance("a.Z");
        let report = declined("meet_types", &base, &other);
        assert_eq!(report.operands, "a.A and a.Z");
        assert_eq!(report.to_string(), "meet_types declined on a.A and a.Z");
    }

    #[test]
    fn a_list_decline_names_every_operand() {
        let items = [instance("a.A"), alias_ref("mod.A")];
        let report = declined_list("make_simplified_union", &items);
        assert_eq!(report.operands, "a.A and alias mod.A");
    }
}
