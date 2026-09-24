//! Standalone-path public API: member.
//!
//! Attribute and member access: instance vs class, properties,
//! descriptors, __getattr__.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of checkmember.rs, checker_helpers.rs, attrs.rs, classmethod_static.rs
//! rather than reimplementing them; where a helper needs a Python-side
//! callback today, take the callback as a Rust trait object or an
//! explicit record and say so in the doc comment.
//!
//! Owned exclusively by the `member` lane for this wave. Add
//! `#[cfg(test)]` unit tests here: they keep the lifted API honest and
//! are the only thing that exercises it before the driver integration
//! wave.
//!
//! # Conventions
//!
//! Every function here is a thin call into the already-ported kernel
//! core; none of them re-derives a mypy algorithm. Each doc comment names
//! the mypy source function it comes from so the differential against the
//! oracle stays readable. The semantic fact store is [`TypeResolver`],
//! the same one the hybrid builds: no area of the standalone path
//! invents its own.
//!
//! `resolver` is always the last parameter, and `Option` is always the
//! kernel's deferral channel: `None` means "the snapshots do not carry
//! enough to decide", never "false". A caller must treat `None` as
//! unsupported-and-loud, not as a negative answer.
//!
//! # Not reachable yet
//!
//! `mypy/checkmember.py::_analyze_member_access` and
//! `analyze_instance_member_access` are deliberately absent from this
//! increment. Their `Instance` arm is `dispatch_instance_member_inner`
//! (checkmember.rs), which reads the receiver's `FuncDef` node live
//! through PyO3 (`get_method_live`, then the `is_property` / `is_static` /
//! `is_trivial_self` / `is_class` / `is_final` flags and `method.type`).
//! A standalone driver has no interpreter to read those from, so exposing
//! the dispatch needs the flags supplied as an explicit Rust record; the
//! pure tails it would call (`static_member_tail`, `member_method_inner`)
//! are already interpreter-free. That record is the next increment.

pub use crate::typeinfo::{TypeInfoSnapshot, TypeResolver};
pub use crate::wire::Type;

/// Which `_analyze_member_access` branch a proper type dispatches to.
///
/// Mirrors the `isinstance` chain in `mypy/checkmember.py:537`
/// (`_analyze_member_access`). The hybrid carries the same decision as an
/// `i64` code (`checkmember::MA_*`) so Python can skip its own isinstance
/// chain; the standalone surface is typed instead, because a caller with
/// no interpreter has no reason to decode a magic integer.
///
/// [`classify_member_access`] is total over the codes the kernel defines:
/// an unmapped code is a drift bug between the two lists and surfaces as
/// `None`, which the caller must reject loudly rather than guess a
/// branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberAccessKind {
    /// `Instance` -> `analyze_instance_member_access`.
    Instance,
    /// `AnyType` -> `AnyType(TypeOfAny.from_another_any)`.
    Any,
    /// `UnionType` -> `analyze_union_member_access`.
    Union,
    /// A `CallableType` / `Overloaded` that `is_type_obj()` ->
    /// `analyze_type_callable_member_access`.
    TypeCallable,
    /// `TypeType` -> `analyze_type_type_member_access`.
    TypeType,
    /// `TupleType` -> recurse on `tuple_fallback`.
    Tuple,
    /// `LiteralType`, or a function-like that is not a type object ->
    /// recurse on the fallback.
    LiteralOrFunc,
    /// `TypedDictType` -> `analyze_typeddict_access`.
    TypedDict,
    /// `NoneType` -> `analyze_none_member_access`.
    NoneType,
    /// `TypeVarType` / `ParamSpecType` / `TypeVarTupleType` -> recurse on
    /// the bound or the tuple fallback.
    TypeVar,
    /// `DeletedType` -> the `deleted_as_rvalue` diagnostic.
    Deleted,
    /// `UninhabitedType` -> a fresh `UninhabitedType`.
    Uninhabited,
    /// `Parameters` / `UnpackType` / `UnboundType` ->
    /// `report_missing_attribute`.
    Missing,
}

/// Map a kernel `MA_*` dispatch code onto [`MemberAccessKind`].
///
/// `None` means the code is not one the kernel defines, i.e. the two
/// lists drifted apart; it is not a mypy answer.
fn kind_from_code(code: i64) -> Option<MemberAccessKind> {
    use crate::checkmember as ma;
    let kind = match code {
        ma::MA_INSTANCE => MemberAccessKind::Instance,
        ma::MA_ANY => MemberAccessKind::Any,
        ma::MA_UNION => MemberAccessKind::Union,
        ma::MA_TYPE_CALLABLE => MemberAccessKind::TypeCallable,
        ma::MA_TYPE_TYPE => MemberAccessKind::TypeType,
        ma::MA_TUPLE => MemberAccessKind::Tuple,
        ma::MA_LITERAL_OR_FUNC => MemberAccessKind::LiteralOrFunc,
        ma::MA_TYPEDDICT => MemberAccessKind::TypedDict,
        ma::MA_NONE => MemberAccessKind::NoneType,
        ma::MA_TYPEVAR => MemberAccessKind::TypeVar,
        ma::MA_DELETED => MemberAccessKind::Deleted,
        ma::MA_UNINHABITED => MemberAccessKind::Uninhabited,
        ma::MA_MISSING => MemberAccessKind::Missing,
        _ => return None,
    };
    Some(kind)
}

/// The `_analyze_member_access` dispatch branch of a type.
///
/// Lifted from `mypy/checkmember.py:537` (`_analyze_member_access`) via
/// `checkmember::classify_member_access_inner`, with the same
/// `get_proper_type` step the hybrid seam performs first.
///
/// Returns `None` for a `TypeAliasType`, whose expansion target the wire
/// format does not carry, exactly as the hybrid defers.
pub fn classify_member_access(typ: &Type, resolver: &TypeResolver) -> Option<MemberAccessKind> {
    let proper = crate::checker_helpers::get_proper_or_none(typ)?;
    let code = crate::checkmember::classify_member_access_inner(proper, resolver);
    kind_from_code(code)
}

/// `mypy/types.py:4218` (`get_proper_type`).
///
/// Lifted from `checker_helpers::get_proper_or_none`. Every type but
/// `TypeAliasType` is already proper; an alias returns `None` because the
/// wire format carries no resolved target, so the caller defers instead
/// of guessing what the alias expands to.
pub fn get_proper_type(typ: &Type) -> Option<&Type> {
    crate::checker_helpers::get_proper_or_none(typ)
}

/// `mypy/types.py:2698` (`CallableType.is_type_obj`).
///
/// Lifted from `checkmember::is_type_obj`: true when the callable's
/// fallback is a metaclass and the return type is not `UninhabitedType`.
/// This decides whether a member access on a function-like type goes to
/// `analyze_type_callable_member_access` or recurses on the fallback.
///
/// There is a second Rust copy at `checker_helpers::is_type_obj` which
/// additionally returns false for a `from_concatenate` callable, a guard
/// upstream `CallableType.is_type_obj()` does not have. Member access
/// uses this copy, matching `checkmember.py`; the divergence is reported,
/// not silently reconciled.
pub fn is_type_obj(fallback: &Type, ret_type: &Type, resolver: &TypeResolver) -> bool {
    crate::checkmember::is_type_obj(fallback, ret_type, resolver)
}

/// `mypy/checkmember.py:2361` (`bind_self_fast`).
///
/// Lifted from `checkmember::bind_self_fast_inner`: strip the first
/// positional argument from a `CallableType` or `Overloaded` and set
/// `is_bound`. A callable with no args, or whose first arg is `*args` /
/// `**kwargs`, is returned unchanged rather than deferred, as in Python.
///
/// Returns `None` for a non-callable type, for an empty `Overloaded`, and
/// for any signature still carrying an `ErasedType`. That last gate is
/// `checkmember::contains_erased`, which the hybrid seam applies before
/// the same call, so binding an erasure placeholder keeps its deferral
/// here too.
pub fn bind_self_fast(typ: &Type) -> Option<Type> {
    if crate::checkmember::contains_erased(typ) {
        return None;
    }
    crate::checkmember::bind_self_fast_inner(typ)
}

/// `mypy/checkmember.py:2443` (`instance_fallback`).
///
/// Lifted from `checkmember::instance_fallback_inner`: `Instance` to
/// itself, `TupleType` to its partial fallback, `LiteralType` /
/// `TypedDictType` to their fallback, anything else to `builtins.object`.
///
/// Returns `None` only for a `TupleType` whose partial fallback is not an
/// `Instance` (the variadic edge), matching the hybrid's deferral.
pub fn instance_fallback(typ: &Type) -> Option<Type> {
    crate::checkmember::instance_fallback_inner(typ)
}

/// `mypy/checkmember.py:2386` (`has_operator`).
///
/// Lifted from `checkmember::has_operator_inner`: whether a proper type
/// has the named operator method, read from the resolver snapshots' MRO
/// and member metadata rather than from live checker state.
///
/// `strict_optional` mirrors `state.strict_optional` for the
/// `relevant_items()` filtering of `NoneType` union items. Returns `None`
/// whenever a snapshot the walk needs is missing, or for a `TypeVarType`
/// with a value restriction, a `ParamSpecType` or a `TypeVarTupleType`,
/// whose `values_or_bound()` the wire format cannot build.
pub fn has_operator(
    typ: &Type,
    op_method: &str,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::checkmember::has_operator_inner(typ, op_method, strict_optional, resolver)
}

/// `mypy/checkmember.py:2456` (`meta_has_operator`).
///
/// Lifted from `checkmember::meta_has_operator_inner`: operator presence
/// on a type's metaclass, defaulting to `builtins.type` when the class
/// declares none. Returns `None` when the class or its metaclass snapshot
/// is missing.
pub fn meta_has_operator(item: &Type, op_method: &str, resolver: &TypeResolver) -> Option<bool> {
    crate::checkmember::meta_has_operator_inner(item, op_method, resolver)
}

/// `mypy/nodes.py:4206` (`TypeInfo.has_readable_member`).
///
/// Lifted from `checkmember::has_readable_member_by_ref`: walk `mro` and
/// report whether some class in it has `name` in its own `names`.
/// Existence only, implicit or explicit, so an instance attribute
/// inferred from `self.x = ...` counts as readable; use
/// [`defined_in_superclass`] for the class-attribute shape.
///
/// `None` when any class in the MRO is missing from the resolver: absent
/// and unknown are not distinguishable, and a wrong boolean here would
/// silently invent or drop a member.
pub fn has_readable_member(type_ref: &str, name: &str, resolver: &TypeResolver) -> Option<bool> {
    crate::checkmember::has_readable_member_by_ref(resolver, type_ref, name)
}

/// `mypy/checkmember.py:2468` (`defined_in_superclass`).
///
/// Lifted from `checkmember::defined_in_superclass_inner`: whether `name`
/// is defined as an explicitly-valued, non-implicit variable in any
/// *superclass* of `fullname`. The class's own MRO entry is skipped, so a
/// member the class defines itself does not count.
///
/// This is the operation that separates the two attribute shapes the
/// snapshot records: `(implicit=false, has_explicit_value=true)` is a
/// class attribute (`kind: str = "shape"`), `(implicit=true, false)` is
/// an instance attribute inferred from `self.x = ...`. A name present
/// only in the instance shape answers `false` here while still answering
/// `true` from [`has_readable_member`].
///
/// `None` when the class or any superclass snapshot is missing.
pub fn defined_in_superclass(fullname: &str, name: &str, resolver: &TypeResolver) -> Option<bool> {
    crate::checkmember::defined_in_superclass_inner(resolver, fullname, name)
}

/// The descriptor-protocol methods a type declares.
///
/// The record form of the `(has_get, has_set)` pair the hybrid seam
/// returns as a tuple, so a caller cannot transpose the two flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorProtocol {
    /// `mypy/checkmember.py:1286` reads `__get__` to decide whether the
    /// access goes through the descriptor tail at all.
    pub has_get: bool,
    /// `__set__` matters only for an lvalue access, where it routes to
    /// `analyze_descriptor_assign`.
    pub has_set: bool,
}

/// The `__get__` / `__set__` presence query behind
/// `mypy/checkmember.py:1286` (`analyze_descriptor_access`).
///
/// Composed from [`has_readable_member`], the same primitive the hybrid
/// seam calls twice; no descriptor logic is re-derived here. Returns
/// `None` for a non-`Instance` proper type or a `TypeAliasType`, and when
/// either MRO walk defers.
///
/// Only the presence question is answered. The descriptor *tail* (binding
/// `__get__`, `map_instance_to_supertype`, `transform_callee_type`,
/// `check_call`) needs checker state and is not part of this increment.
pub fn descriptor_has_get_set(
    descriptor: &Type,
    resolver: &TypeResolver,
) -> Option<DescriptorProtocol> {
    let proper = crate::checker_helpers::get_proper_or_none(descriptor)?;
    let type_ref = match proper {
        Type::Instance { type_ref, .. } => type_ref.as_str(),
        _ => return None,
    };
    let has_get = has_readable_member(type_ref, "__get__", resolver)?;
    let has_set = has_readable_member(type_ref, "__set__", resolver)?;
    Some(DescriptorProtocol { has_get, has_set })
}

/// `mypy/subtypes.py:2716` (`is_descriptor`).
///
/// Lifted from `checker_helpers::is_descriptor_wire`: true iff the type is
/// an `Instance` whose class has a readable `__get__`, or a `UnionType`
/// with such an item. `None` when any component cannot be decided from
/// the snapshots.
pub fn is_descriptor(typ: &Type, resolver: &TypeResolver) -> Option<bool> {
    crate::checker_helpers::is_descriptor_wire(typ, resolver)
}

/// The `__bool__` arm of `mypy/checkmember.py:1113`
/// (`analyze_none_member_access`).
///
/// Lifted from `checkmember::analyze_none_bool_type`: `None.__bool__` is a
/// zero-argument callable returning `Literal[False]` with a
/// `builtins.bool` fallback. This is the one member of `NoneType` mypy
/// answers without recursing to `builtins.object`, so a driver that
/// treated `None` uniformly would mis-answer it.
pub fn none_bool_method_type() -> Type {
    crate::checkmember::analyze_none_bool_type()
}

/// `mypy/types.py:4114` (`TypeType.make_normalized`).
///
/// Lifted from `checkmember::make_type_type_normalized`: wrap a type in
/// `TypeType`, distributing over a `UnionType`'s items so a union of
/// classes becomes a union of class objects rather than one class object
/// of a union.
pub fn make_type_type_normalized(item: &Type) -> Type {
    crate::checkmember::make_type_type_normalized(item)
}

/// `mypy/typeops.py:2272` (`custom_special_method`).
///
/// Lifted from `checker_helpers::custom_special_method_inner`: whether the
/// type has a custom special method such as `__eq__`, where "custom"
/// means the defining class is not under `builtins.` or `typing.`.
/// `check_all` selects union semantics (`all` rather than `any`).
///
/// Defers on a `TypeAliasType`: the hybrid seam expands aliases through
/// its alias resolver before this call, and no alias store is threaded
/// through the standalone path yet. A deferral is safe here because it
/// cannot produce a wrong boolean.
pub fn custom_special_method(
    typ: &Type,
    name: &str,
    check_all: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::checker_helpers::custom_special_method_inner(typ, name, check_all, resolver)
}

/// The `Instance` arm of `mypy/typeops.py:2272` (`custom_special_method`).
///
/// Lifted from `checker_helpers::instance_custom_special_method`: the MRO
/// walk behind [`custom_special_method`], exposed on its own because a
/// driver that already holds a class fullname should not have to build an
/// `Instance` to ask the question. `None` when any MRO snapshot is
/// missing.
pub fn instance_custom_special_method(
    type_ref: &str,
    name: &str,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::checker_helpers::instance_custom_special_method(type_ref, name, resolver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::LiteralValue;

    /// `ArgKind.ARG_POS` / `ARG_STAR` (checkmember.rs).
    const ARG_POS: i64 = 0;
    const ARG_STAR: i64 = 2;

    fn inst(type_ref: &str) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args: vec![],
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn any_type() -> Type {
        Type::AnyType {
            type_of_any: 1,
            source_any: None,
            missing_import_name: None,
        }
    }

    fn alias() -> Type {
        Type::TypeAliasType {
            args: vec![],
            type_ref: "mod.Alias".to_string(),
            is_recursive: false,
        }
    }

    fn tuple_of(partial_fallback: Type, items: Vec<Type>) -> Type {
        Type::TupleType {
            partial_fallback: Box::new(partial_fallback),
            items,
            implicit: false,
        }
    }

    fn union(items: Vec<Type>) -> Type {
        Type::UnionType {
            items,
            uses_pep604_syntax: false,
            can_be_true: true,
            can_be_false: true,
            is_evaluated: true,
            original_str_expr: None,
            original_str_fallback: None,
        }
    }

    /// A one-argument method whose first parameter is `self`.
    fn method(first_arg_kind: i64) -> Type {
        Type::CallableType {
            fallback: Box::new(inst("builtins.function")),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: false,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs: false,
            from_type_type: false,
            arg_types: vec![inst("C")],
            arg_kinds: vec![first_arg_kind],
            arg_names: vec![Some("self".to_string())],
            ret_type: Box::new(inst("builtins.int")),
            name: Some("method".to_string()),
            variables: vec![],
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    /// What `bind_self_fast` must produce from [`method`]: the self
    /// argument stripped and `is_bound` set.
    fn bound_method() -> Type {
        Type::CallableType {
            fallback: Box::new(inst("builtins.function")),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: true,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs: false,
            from_type_type: false,
            arg_types: vec![],
            arg_kinds: vec![],
            arg_names: vec![],
            ret_type: Box::new(inst("builtins.int")),
            name: Some("method".to_string()),
            variables: vec![],
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    /// A snapshot whose MRO is `mro`, with `members` installed on the
    /// class itself as `(implicit, has_explicit_value)` pairs.
    fn class(fullname: &str, mro: &[&str], members: &[(&str, (bool, bool))]) -> TypeInfoSnapshot {
        let mut s = TypeInfoSnapshot {
            fullname: fullname.to_string(),
            name: fullname.to_string(),
            ..Default::default()
        };
        for entry in mro {
            s.mro.push((*entry).to_string());
            s.has_base.insert((*entry).to_string());
        }
        for (name, flags) in members {
            s.member_info.insert((*name).to_string(), *flags);
        }
        s
    }

    /// A resolver holding `builtins.object`, `builtins.type` and `extra`.
    fn resolver_with(extra: Vec<TypeInfoSnapshot>) -> TypeResolver {
        let mut r = TypeResolver::new();
        let object = class("builtins.object", &["builtins.object"], &[]);
        r.insert("builtins.object".to_string(), object);
        let meta = class("builtins.type", &["builtins.type", "builtins.object"], &[]);
        r.insert("builtins.type".to_string(), meta);
        for s in extra {
            let key = s.fullname.clone();
            r.insert(key, s);
        }
        r
    }

    /// Record `name` as a method of the snapshot, defined by `definer`.
    /// Node kind 0 is `FuncBase`, the kind `member_definers` uses for a
    /// `FuncDef` / `OverloadedFuncDef`.
    fn with_definer(mut s: TypeInfoSnapshot, name: &str, definer: &str) -> TypeInfoSnapshot {
        let entry = (0i64, definer.to_string());
        s.member_definers.insert(name.to_string(), entry);
        s
    }

    fn arg_count(typ: &Type) -> usize {
        match typ {
            Type::CallableType { arg_types, .. } => arg_types.len(),
            other => panic!("expected a CallableType, got {other:?}"),
        }
    }

    fn ret_type(typ: &Type) -> &Type {
        match typ {
            Type::CallableType { ret_type, .. } => ret_type,
            other => panic!("expected a CallableType, got {other:?}"),
        }
    }

    fn is_literal_false(typ: &Type) -> bool {
        match typ {
            Type::LiteralType { value, .. } => *value == LiteralValue::Bool(false),
            _ => false,
        }
    }

    fn instance_ref(typ: &Type) -> String {
        match typ {
            Type::Instance { type_ref, .. } => type_ref.clone(),
            other => panic!("expected an Instance, got {other:?}"),
        }
    }

    fn literal_fallback(typ: &Type) -> String {
        match typ {
            Type::LiteralType { fallback, .. } => instance_ref(fallback),
            other => panic!("expected a LiteralType, got {other:?}"),
        }
    }

    fn type_type_of(item: Type) -> Type {
        Type::TypeType {
            item: Box::new(item),
            is_type_form: false,
        }
    }

    // -- classify_member_access -----------------------------------------

    #[test]
    fn classify_dispatches_the_instance_and_singleton_branches() {
        let r = resolver_with(vec![class("C", &["C", "builtins.object"], &[])]);
        let instance = classify_member_access(&inst("C"), &r);
        assert_eq!(instance, Some(MemberAccessKind::Instance));
        let none = classify_member_access(&Type::NoneType, &r);
        assert_eq!(none, Some(MemberAccessKind::NoneType));
        let any = classify_member_access(&any_type(), &r);
        assert_eq!(any, Some(MemberAccessKind::Any));
    }

    /// Rejection: an alias has no expansion target on this path.
    #[test]
    fn classify_defers_on_an_alias() {
        let r = resolver_with(vec![]);
        assert_eq!(classify_member_access(&alias(), &r), None);
    }

    /// Negative control on the code mapping: every code the kernel defines
    /// must land on a variant, so a new `MA_*` constant cannot be silently
    /// dropped into the `_ => None` arm. Deleting any mapping arm above
    /// turns this red.
    #[test]
    fn kind_mapping_covers_every_kernel_code() {
        use crate::checkmember as ma;
        let codes = [
            ma::MA_INSTANCE,
            ma::MA_ANY,
            ma::MA_UNION,
            ma::MA_TYPE_CALLABLE,
            ma::MA_TYPE_TYPE,
            ma::MA_TUPLE,
            ma::MA_LITERAL_OR_FUNC,
            ma::MA_TYPEDDICT,
            ma::MA_NONE,
            ma::MA_TYPEVAR,
            ma::MA_DELETED,
            ma::MA_UNINHABITED,
            ma::MA_MISSING,
        ];
        assert_eq!(codes.len(), 13, "the kernel defines 13 codes");
        for code in codes {
            let mapped = kind_from_code(code);
            assert!(mapped.is_some(), "code {code} has no kind");
        }
        assert_eq!(kind_from_code(ma::MA_MISSING + 1), None);
    }

    // -- get_proper_type ------------------------------------------------

    #[test]
    fn proper_type_passes_through_and_defers_on_an_alias() {
        let t = inst("C");
        assert_eq!(get_proper_type(&t), Some(&t));
        let a = alias();
        assert_eq!(get_proper_type(&a), None);
    }

    // -- is_type_obj ----------------------------------------------------

    #[test]
    fn type_obj_needs_a_metaclass_fallback() {
        let r = resolver_with(vec![]);
        let c = inst("C");
        assert!(is_type_obj(&inst("builtins.type"), &c, &r));
        assert!(!is_type_obj(&inst("builtins.function"), &c, &r));
    }

    #[test]
    fn type_obj_rejects_an_uninhabited_return() {
        let r = resolver_with(vec![]);
        let never = Type::UninhabitedType { ambiguous: false };
        let meta = inst("builtins.type");
        assert!(!is_type_obj(&meta, &never, &r));
    }

    // -- bind_self_fast -------------------------------------------------

    #[test]
    fn bind_self_strips_the_first_positional_argument() {
        let m = method(ARG_POS);
        assert_eq!(bind_self_fast(&m), Some(bound_method()));
    }

    #[test]
    fn bind_self_leaves_a_star_args_callable_unchanged() {
        let m = method(ARG_STAR);
        assert_eq!(bind_self_fast(&m), Some(m.clone()));
    }

    /// Rejection: a non-callable has no self argument to strip.
    #[test]
    fn bind_self_rejects_a_non_callable() {
        assert_eq!(bind_self_fast(&inst("C")), None);
    }

    #[test]
    fn bind_self_rejects_an_erased_signature() {
        let mut m = method(ARG_POS);
        if let Type::CallableType { arg_types, .. } = &mut m {
            arg_types.push(Type::ErasedType);
        }
        assert_eq!(
            bind_self_fast(&m),
            None,
            "an ErasedType placeholder must keep its deferral"
        );
    }

    // -- instance_fallback ----------------------------------------------

    #[test]
    fn instance_fallback_of_a_tuple_is_its_partial_fallback() {
        let items = vec![inst("builtins.int")];
        let t = tuple_of(inst("builtins.tuple"), items);
        let fb = instance_fallback(&t);
        assert_eq!(fb, Some(inst("builtins.tuple")));
    }

    #[test]
    fn instance_fallback_of_an_unrelated_type_is_object() {
        let fb = instance_fallback(&Type::NoneType);
        assert_eq!(fb, Some(inst("builtins.object")));
    }

    /// Rejection: the variadic edge, where the partial fallback is not an
    /// `Instance`.
    #[test]
    fn instance_fallback_defers_on_a_non_instance_tuple_fallback() {
        let t = tuple_of(Type::NoneType, vec![]);
        assert_eq!(instance_fallback(&t), None);
    }

    // -- has_operator / meta_has_operator -------------------------------

    #[test]
    fn has_operator_reads_the_mro() {
        let add = [("__add__", (false, false))];
        let c = class("C", &["C", "builtins.object"], &add);
        let r = resolver_with(vec![c]);
        let found = has_operator(&inst("C"), "__add__", true, &r);
        assert_eq!(found, Some(true));
    }

    /// Rejection: an instance whose class declares no such member.
    #[test]
    fn has_operator_is_false_when_the_member_is_absent() {
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![c]);
        let missing = has_operator(&inst("C"), "__add__", true, &r);
        assert_eq!(missing, Some(false));
    }

    #[test]
    fn has_operator_defers_on_a_missing_snapshot() {
        let r = resolver_with(vec![]);
        let unknown = has_operator(&inst("C"), "__add__", true, &r);
        assert_eq!(unknown, None);
    }

    #[test]
    fn meta_has_operator_reads_the_declared_metaclass() {
        let or = [("__or__", (false, false))];
        let m = class("M", &["M", "builtins.type"], &or);
        let mut c = class("C", &["C", "builtins.object"], &[]);
        c.metaclass_fullname = Some("M".to_string());
        let r = resolver_with(vec![m, c]);
        let found = meta_has_operator(&inst("C"), "__or__", &r);
        assert_eq!(found, Some(true));
        let absent = meta_has_operator(&inst("C"), "__and__", &r);
        assert_eq!(absent, Some(false));
    }

    #[test]
    fn meta_has_operator_defaults_to_builtins_type() {
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![c]);
        let found = meta_has_operator(&inst("C"), "__call__", &r);
        assert_eq!(found, Some(false));
    }

    // -- has_readable_member / defined_in_superclass --------------------

    #[test]
    fn readable_member_walks_the_mro() {
        let value = [("value", (false, true))];
        let b = class("B", &["B", "builtins.object"], &value);
        let c = class("C", &["C", "B", "builtins.object"], &[]);
        let r = resolver_with(vec![b, c]);
        assert_eq!(has_readable_member("C", "value", &r), Some(true));
    }

    /// Rejection: an instance with no such member anywhere in its MRO.
    #[test]
    fn readable_member_is_false_when_no_class_declares_it() {
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![c]);
        assert_eq!(has_readable_member("C", "missing", &r), Some(false));
    }

    #[test]
    fn readable_member_defers_on_a_missing_mro_entry() {
        let c = class("C", &["C", "mod.Absent"], &[]);
        let r = resolver_with(vec![c]);
        assert_eq!(has_readable_member("C", "value", &r), None);
    }

    #[test]
    fn defined_in_superclass_finds_a_class_attribute() {
        let shape = [("shape", (false, true))];
        let b = class("B", &["B", "builtins.object"], &shape);
        let c = class("C", &["C", "B", "builtins.object"], &[]);
        let r = resolver_with(vec![b, c]);
        let found = defined_in_superclass("C", "shape", &r);
        assert_eq!(found, Some(true));
    }

    /// Rejection: the class-attribute-versus-instance-attribute mismatch.
    /// The name is readable, but as an implicit instance attribute it is
    /// not a superclass class attribute. Dropping the `!*implicit`
    /// condition in the lifted walk turns this red.
    #[test]
    fn defined_in_superclass_rejects_an_instance_attribute() {
        let shape = [("shape", (true, false))];
        let b = class("B", &["B", "builtins.object"], &shape);
        let c = class("C", &["C", "B", "builtins.object"], &[]);
        let r = resolver_with(vec![b, c]);
        assert_eq!(has_readable_member("C", "shape", &r), Some(true));
        let found = defined_in_superclass("C", "shape", &r);
        assert_eq!(found, Some(false));
    }

    #[test]
    fn defined_in_superclass_skips_the_classs_own_entry() {
        let shape = [("shape", (false, true))];
        let c = class("C", &["C", "builtins.object"], &shape);
        let r = resolver_with(vec![c]);
        let found = defined_in_superclass("C", "shape", &r);
        assert_eq!(found, Some(false));
    }

    // -- descriptor protocol --------------------------------------------

    #[test]
    fn descriptor_flags_report_get_without_set() {
        let get = [("__get__", (false, false))];
        let d = class("D", &["D", "builtins.object"], &get);
        let r = resolver_with(vec![d]);
        let flags = descriptor_has_get_set(&inst("D"), &r);
        let expected = Some(DescriptorProtocol {
            has_get: true,
            has_set: false,
        });
        assert_eq!(flags, expected);
        assert_eq!(is_descriptor(&inst("D"), &r), Some(true));
    }

    /// Rejection: a plain instance is not a descriptor.
    #[test]
    fn descriptor_flags_reject_a_plain_instance() {
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![c]);
        let flags = descriptor_has_get_set(&inst("C"), &r);
        let expected = Some(DescriptorProtocol {
            has_get: false,
            has_set: false,
        });
        assert_eq!(flags, expected);
        assert_eq!(is_descriptor(&inst("C"), &r), Some(false));
    }

    #[test]
    fn descriptor_flags_reject_a_non_instance() {
        let r = resolver_with(vec![]);
        assert_eq!(descriptor_has_get_set(&Type::NoneType, &r), None);
        let a = alias();
        assert_eq!(descriptor_has_get_set(&a, &r), None);
    }

    // -- NoneType __bool__ ----------------------------------------------

    #[test]
    fn none_bool_is_a_zero_arg_callable_returning_literal_false() {
        let result = none_bool_method_type();
        assert_eq!(arg_count(&result), 0, "no arguments");
        let ret = ret_type(&result);
        assert!(is_literal_false(ret), "must return Literal[False]");
        assert_eq!(literal_fallback(ret), "builtins.bool");
    }

    // -- make_type_type_normalized --------------------------------------

    #[test]
    fn make_type_type_wraps_a_plain_item() {
        let expected = type_type_of(inst("C"));
        assert_eq!(make_type_type_normalized(&inst("C")), expected);
    }

    #[test]
    fn make_type_type_distributes_over_a_union() {
        let u = union(vec![inst("A"), inst("B")]);
        let expected = union(vec![type_type_of(inst("A")), type_type_of(inst("B"))]);
        assert_eq!(make_type_type_normalized(&u), expected);
    }

    // -- custom_special_method ------------------------------------------

    #[test]
    fn custom_special_method_true_for_a_user_definer() {
        let c = with_definer(class("C", &["C", "builtins.object"], &[]), "__eq__", "C");
        let r = resolver_with(vec![c]);
        let custom = custom_special_method(&inst("C"), "__eq__", false, &r);
        assert_eq!(custom, Some(true));
        let by_ref = instance_custom_special_method("C", "__eq__", &r);
        assert_eq!(by_ref, Some(true));
    }

    /// Rejection: a member defined by `builtins` is not custom.
    #[test]
    fn custom_special_method_false_for_a_builtin_definer() {
        let bare = class("C", &["C", "builtins.object"], &[]);
        let c = with_definer(bare, "__eq__", "builtins.object");
        let r = resolver_with(vec![c]);
        let custom = custom_special_method(&inst("C"), "__eq__", false, &r);
        assert_eq!(custom, Some(false));
        let by_ref = instance_custom_special_method("C", "__eq__", &r);
        assert_eq!(by_ref, Some(false));
    }

    #[test]
    fn custom_special_method_defers_without_the_snapshot() {
        let r = resolver_with(vec![]);
        let by_ref = instance_custom_special_method("C", "__eq__", &r);
        assert_eq!(by_ref, None);
        let a = alias();
        assert_eq!(custom_special_method(&a, "__eq__", false, &r), None);
    }
}
