//! Standalone-path public API: semanal.
//!
//! Semantic analysis: name binding, scopes, symbol tables, type-
//! expression analysis, base and metaclass resolution.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of semanal_visitor.rs, semanal_checks.rs, semanal_shared.rs,
//! semanal_bases.rs, semanal_algebra.rs, semanal_metaclass.rs,
//! semanal_classprop.rs, semanal_lookup.rs, semanal_typeddict.rs,
//! semanal_typeexpr.rs, typeanal_queries.rs, typeanal_special.rs,
//! typeanal_callable.rs, typeanal_info.rs, typeanal_literal.rs,
//! typeanal_deprec.rs, binder.rs rather than reimplementing them; where a
//! helper needs a Python-side callback today, take the callback as a Rust
//! trait object or an explicit record and say so in the doc comment.
//!
//! Owned exclusively by the `semanal` lane for this wave. Add
//! `#[cfg(test)]` unit tests here: they keep the lifted API honest and
//! are the only thing that exercises it before the driver integration
//! wave.
//!
//! # What is reachable so far
//!
//! Each entry below names the mypy function it ports and the kernel
//! module the logic is lifted from, so the differential against the
//! oracle stays readable. Nothing here decides anything the hybrid
//! decides differently: every function is the identical code path the
//! `#[pyfunction]` seam calls, minus the wire decode/encode round trip.
//!
//! Three families are exposed by this increment:
//!
//! * the `Any`-rewriting algebra and the type queries of
//!   `mypy/semanal.py` and `mypy/typeanal.py`, which the driver needs to
//!   gate `--disallow-any-explicit` / `--disallow-any-unimported` on a
//!   type alias target and to bind a method's implicit first argument;
//! * the two name-elision predicates of `mypy/sharedparse.py`, which
//!   decide the parameter names a signature displays;
//! * the two record-decidable arms of
//!   `SemanticAnalyzer.lookup_qualified`, the TypeInfo MRO step and the
//!   MypyFile symbol-table chain.
//!
//! # Deferral is a hard error here, not a fallback
//!
//! Several lifted queries return `Option`. In the hybrid `None` means
//! "run the pure-Python body instead", because the wire type alone cannot
//! decide (a `TypeAliasType` whose target only the live alias resolver
//! holds). The standalone path has no Python body to fall back to, so a
//! `None` is a records-coverage failure and the caller must reject loudly
//! naming the construct, exactly as `Driver::require_decidable` does for
//! `is_subtype`. The resolver-backed `*_live` variants that expand
//! aliases are not reachable yet: they take
//! `typeinfo::NativeTypeResolver`, a `#[pyclass]`, and
//! `aliases::TypeAliasResolver` is crate-private.

pub use crate::skeleton_api::{ModuleSnapshot, Type, TypeInfoSnapshot, TypeResolver};

/// `mypy.semanal.make_any_non_explicit` (semanal.py:10161).
///
/// Rewrites every `AnyType(type_of_any=explicit)` inside `t` to
/// `AnyType(special_form)`, because an inlined type-alias target is no
/// longer an explicit `Any`. Lifts
/// `semanal_algebra::make_any_non_explicit_inner`, the same code the
/// `rust_make_any_non_explicit` seam wraps.
pub fn make_any_non_explicit(t: Type) -> Type {
    crate::semanal_algebra::make_any_non_explicit_inner(t)
}

/// `mypy.semanal.make_any_non_unimported` (semanal.py:10188).
///
/// Rewrites every `AnyType(type_of_any=from_unimported_type)` inside `t`
/// to `AnyType(special_form)`, and clears `missing_import_name` on every
/// `AnyType` it walks whether or not it rewrote it. Lifts
/// `semanal_algebra::make_any_non_unimported_inner`.
pub fn make_any_non_unimported(t: Type) -> Type {
    crate::semanal_algebra::make_any_non_unimported_inner(t)
}

/// `mypy.semanal.replace_implicit_first_type` (semanal.py:9994).
///
/// Swaps the first (implicit `self` / `cls`) argument type of a
/// `FunctionLike` for `new`, preserving every other field; an `Overloaded`
/// recurses into each item. `SemanticAnalyzer.prepare_method_signature`
/// (semanal.py:1660) uses it to install the annotated `self` type.
///
/// `None` means the input was neither a `CallableType` nor an `Overloaded`
/// of them. mypy never passes one, so there is no Python body to fall back
/// to: the caller must reject loudly. Lifts
/// `semanal_algebra::replace_implicit_first_type_inner`.
pub fn replace_implicit_first_type(sig: Type, new: &Type) -> Option<Type> {
    crate::semanal_algebra::replace_implicit_first_type_inner(sig, new)
}

/// `mypy.typeanal.has_explicit_any` (typeanal.py:4276).
///
/// `Some(true)` when `t` is, or contains, an `AnyType` whose
/// `type_of_any` is `explicit`. A `TypedDictType` subtree always answers
/// `Some(false)`, mirroring `HasExplicitAny.visit_typeddict_type`: a
/// TypedDict is checked where it is declared, not here.
///
/// `None` is the deferral described in the module docs: the walk reached a
/// `TypeAliasType` whose target the wire type does not carry. Lifts
/// `typeanal_queries::has_explicit_any_inner`.
pub fn has_explicit_any(t: &Type) -> Option<bool> {
    let explicit = crate::typeanal_queries::EXPLICIT;
    crate::typeanal_queries::has_explicit_any_inner(t, explicit)
}

/// `mypy.typeanal.has_any_from_unimported_type` (typeanal.py:4307).
///
/// The same `ANY_STRATEGY` walk as [`has_explicit_any`], matching
/// `type_of_any == from_unimported_type` instead. `None` defers on a
/// `TypeAliasType`. Lifts `typeanal_queries::has_explicit_any_inner`.
pub fn has_any_from_unimported_type(t: &Type) -> Option<bool> {
    let unimported = crate::typeanal_queries::FROM_UNIMPORTED_TYPE;
    crate::typeanal_queries::has_explicit_any_inner(t, unimported)
}

/// `mypy.typeanal.check_for_explicit_any` (typeanal.py:3805).
///
/// The option gate around [`has_explicit_any`]: mypy emits
/// `msg.explicit_any` only when `disallow_any_explicit` is set and the
/// file is not a typeshed stub. `Some(true)` means the report fires,
/// `Some(false)` that it does not, and `None` that the gate is open but
/// the type contains an alias the records cannot expand.
///
/// Ported here rather than lifted: the kernel carries only the raw walk,
/// the gate itself exists solely in Python today. The `typ` truthiness
/// test of the Python original is absent because this API takes a `&Type`,
/// never an optional one.
pub fn check_for_explicit_any(
    t: &Type,
    disallow_any_explicit: bool,
    is_typeshed_stub: bool,
) -> Option<bool> {
    if !disallow_any_explicit || is_typeshed_stub {
        return Some(false);
    }
    has_explicit_any(t)
}

/// `mypy.typeanal.collect_all_inner_types` (typeanal.py:4343).
///
/// `CollectAllInnerTypesQuery`: the direct children of `t`, followed by
/// their children recursively, excluding `t` itself. `None` defers on a
/// `TypeAliasType`. Lifts `typeanal_queries::collect_all_inner_types_inner`.
pub fn collect_all_inner_types(t: &Type) -> Option<Vec<Type>> {
    crate::typeanal_queries::collect_all_inner_types_inner(t)
}

/// `mypy.typeanal.unknown_unpack` (typeanal.py:4511).
///
/// `Some(true)` when `t` is an `UnpackType` whose target is an
/// `AnyType(special_form)`, the shape mypy builds for an `Unpack` it could
/// not resolve. Anything that is not an `UnpackType` answers
/// `Some(false)`; `None` defers when the unpacked target is an alias.
/// Lifts `typeanal_queries::unknown_unpack_inner`.
pub fn unknown_unpack(t: &Type) -> Option<bool> {
    crate::typeanal_queries::unknown_unpack_inner(t)
}

/// `mypy.typeanal.SELF_TYPE_NAMES` membership (typeanal.py:146).
///
/// Whether `fullname` names the `Self` special form, i.e. is
/// `typing.Self` or `typing_extensions.Self`. Lifts
/// `typeanal_queries::is_self_fullname`.
pub fn is_self_type_fullname(fullname: &str) -> bool {
    crate::typeanal_queries::is_self_fullname(fullname)
}

/// `mypy.messages.wrong_type_arg_count` (messages.py:4073).
///
/// The type-argument arity message `TypeAnalyser` emits for a bad
/// subscript, e.g. `"Box" expects 1 type argument, but 2 given`. A `given`
/// of zero renders as `none`, mirroring the `act == "0"` normalization at
/// messages.py:4088; `min != max` renders the `between {min} and {max}`
/// form. Lifts `typeanal_queries::wrong_type_arg_count_msg`, the copy
/// `TypeAnalyser`'s native arity path calls.
pub fn wrong_type_arg_count_msg(min: usize, max: usize, given: usize, type_name: &str) -> String {
    crate::typeanal_queries::wrong_type_arg_count_msg(min, max, given, type_name)
}

/// `mypy.sharedparse.special_function_elide_names` (sharedparse.py:109).
///
/// Whether `name` is a magic method whose parameters mypy marks
/// positional-only: `NON_BINARY_MAGIC_METHODS | BINARY_MAGIC_METHODS`
/// minus `MAGIC_METHODS_ALLOWING_KWARGS`. Lifts
/// `semanal_shared::special_function_elide_names_inner`.
pub fn special_function_elide_names(name: &str) -> bool {
    crate::semanal_shared::special_function_elide_names_inner(name)
}

/// `mypy.sharedparse.argument_elide_name` (sharedparse.py:113).
///
/// Whether a parameter named `name` is elided from a signature: a leading
/// dunder that is not also a trailing one. `None` (an unnamed parameter)
/// is never elided. Lifts `semanal_shared::argument_elide_name_inner`.
pub fn argument_elide_name(name: Option<&str>) -> bool {
    crate::semanal_shared::argument_elide_name_inner(name)
}

/// The answer one `SemanticAnalyzer.lookup_qualified` dot-chain step
/// (semanal.py:7126-7181) can give a caller that holds records instead of
/// live Python symbols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupOutcome {
    /// The chain resolved. Carries the fullname of the namespace the final
    /// part lives in: the defining MRO entry for a class member, the
    /// module for a symbol-table chain.
    Resolved(String),
    /// Positively absent. mypy reports "name not defined"; the standalone
    /// driver renders the same diagnostic.
    NotFound,
    /// The records cannot decide: a missing snapshot, or a chain step the
    /// kernel only answers from live Python. This is the deferral of the
    /// module docs, so the caller must reject loudly.
    Deferred,
}

/// The TypeInfo arm of `SemanticAnalyzer.lookup_qualified`
/// (semanal.py:7126-7181): resolve `name` against the MRO of
/// `class_fullname`, mirroring `TypeInfo.get(name)` (nodes.py).
///
/// `LookupOutcome::Resolved` carries the fullname of the MRO entry that
/// defines the member, which is what mypy's override and attribute checks
/// compare against. `Deferred` means `class_fullname` has no snapshot in
/// `resolver`; `NotFound` means no MRO entry carries the name.
///
/// Lifts `semanal_lookup::find_member_in_mro`. The full seam
/// (`rust_lookup_qualified`) additionally handles the `PlaceholderNode`,
/// `Var`, `TypeAlias` and `ParamSpecExpr` first-symbol arms, all of which
/// need `NativeTypeResolver` plus live Python symbol nodes and are
/// therefore out of reach here.
pub fn lookup_typeinfo_member(
    resolver: &TypeResolver,
    class_fullname: &str,
    name: &str,
) -> LookupOutcome {
    let Some(snap) = resolver.get(class_fullname) else {
        return LookupOutcome::Deferred;
    };
    match crate::semanal_lookup::find_member_in_mro(resolver, snap, name) {
        Some(defining) => LookupOutcome::Resolved(defining),
        None => LookupOutcome::NotFound,
    }
}

/// The MypyFile arm of `SemanticAnalyzer.lookup_qualified`
/// (semanal.py:7126-7181): walk `dotted_name` through module symbol
/// tables, mirroring the per-step `get_module_symbol(node, part)` call.
///
/// `dotted_name` is the whole dotted name including the already-resolved
/// first part, exactly as `lookup_qualified` receives it: the walk starts
/// at `parts[1]`. `module_fullname` is the fullname of the module the
/// first part resolved to, which need not be `parts[0]` (an aliased
/// `import x as y` descends by the symbol node's own fullname).
///
/// `LookupOutcome::Resolved` carries the fullname of the module whose
/// namespace holds the final part. Lifts
/// `semanal_lookup::walk_mypyfile_chain`.
pub fn lookup_module_chain(
    resolver: &TypeResolver,
    module_fullname: &str,
    dotted_name: &str,
) -> LookupOutcome {
    let parts: Vec<&str> = dotted_name.split('.').collect();
    match crate::semanal_lookup::walk_mypyfile_chain(resolver, &parts, module_fullname) {
        crate::semanal_lookup::WalkOutcome::Resolved(full) => LookupOutcome::Resolved(full),
        crate::semanal_lookup::WalkOutcome::NotFound => LookupOutcome::NotFound,
        crate::semanal_lookup::WalkOutcome::Defer => LookupOutcome::Deferred,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typeanal_queries::{EXPLICIT, FROM_UNIMPORTED_TYPE};
    use std::collections::HashMap;

    /// `TypeOfAny.from_error` (mypy/types.py:213-239). The kernel keeps
    /// this constant private, so the test names it.
    const FROM_ERROR: i64 = 5;
    /// `TypeOfAny.special_form` (mypy/types.py:213-239).
    const SPECIAL_FORM: i64 = 6;

    fn any(type_of_any: i64) -> Type {
        Type::AnyType {
            type_of_any,
            source_any: None,
            missing_import_name: None,
        }
    }

    fn any_with(type_of_any: i64, import: &str) -> Type {
        Type::AnyType {
            type_of_any,
            source_any: None,
            missing_import_name: Some(import.to_string()),
        }
    }

    fn instance(type_ref: &str, args: Vec<Type>) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args,
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn alias(type_ref: &str) -> Type {
        Type::TypeAliasType {
            args: Vec::new(),
            type_ref: type_ref.to_string(),
            is_recursive: false,
        }
    }

    fn unpack(target: Type) -> Type {
        Type::UnpackType {
            typ: Box::new(target),
            from_star_syntax: false,
        }
    }

    fn callable(arg_types: Vec<Type>) -> Type {
        let fallback = instance("builtins.function", vec![any(SPECIAL_FORM)]);
        Type::CallableType {
            fallback: Box::new(fallback),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: false,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs: false,
            from_type_type: false,
            arg_types,
            arg_kinds: Vec::new(),
            arg_names: Vec::new(),
            ret_type: Box::new(Type::NoneType),
            name: None,
            variables: Vec::new(),
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    fn any_kind(t: &Type) -> i64 {
        match t {
            Type::AnyType { type_of_any, .. } => *type_of_any,
            other => panic!("expected an AnyType, got {other:?}"),
        }
    }

    fn any_import(t: &Type) -> Option<&str> {
        match t {
            Type::AnyType { missing_import_name, .. } => missing_import_name.as_deref(),
            other => panic!("expected an AnyType, got {other:?}"),
        }
    }

    fn arg_types(t: &Type) -> &[Type] {
        match t {
            Type::CallableType { arg_types, .. } => arg_types,
            other => panic!("expected a CallableType, got {other:?}"),
        }
    }

    fn class(fullname: &str, mro: &[&str], members: &[&str]) -> TypeInfoSnapshot {
        let mut member_info = HashMap::new();
        for name in members {
            member_info.insert(name.to_string(), (false, false));
        }
        TypeInfoSnapshot {
            fullname: fullname.to_string(),
            mro: mro.iter().map(|m| m.to_string()).collect(),
            member_info,
            ..Default::default()
        }
    }

    fn module(entries: &[(&str, bool, Option<&str>)]) -> ModuleSnapshot {
        let mut symbols = HashMap::new();
        for &(name, hidden, nested) in entries {
            let node = nested.map(|f| (true, f.to_string()));
            symbols.insert(name.to_string(), (hidden, node));
        }
        ModuleSnapshot { symbols }
    }

    /// `mod.Sub` extends `mod.Base`; only the base defines `base_method`.
    fn mro_resolver() -> TypeResolver {
        let mut resolver = TypeResolver::new();
        let base = class("mod.Base", &["mod.Base"], &["base_method"]);
        resolver.insert("mod.Base".to_string(), base);
        let sub = class("mod.Sub", &["mod.Sub", "mod.Base"], &[]);
        resolver.insert("mod.Sub".to_string(), sub);
        resolver
    }

    #[test]
    fn make_any_non_explicit_rewrites_an_explicit_any() {
        assert_eq!(make_any_non_explicit(any(EXPLICIT)), any(SPECIAL_FORM));
    }

    #[test]
    fn make_any_non_explicit_rejects_a_from_error_any() {
        let kept = make_any_non_explicit(any(FROM_ERROR));
        assert_eq!(any_kind(&kept), FROM_ERROR);
    }

    #[test]
    fn make_any_non_unimported_rewrites_and_drops_the_import_name() {
        let rewritten = make_any_non_unimported(any_with(FROM_UNIMPORTED_TYPE, "missing"));
        assert_eq!(rewritten, any(SPECIAL_FORM));
    }

    #[test]
    fn make_any_non_unimported_keeps_an_explicit_any() {
        let rewritten = make_any_non_unimported(any_with(EXPLICIT, "missing"));
        assert_eq!(any_kind(&rewritten), EXPLICIT);
        assert_eq!(any_import(&rewritten), None);
    }

    #[test]
    fn replace_implicit_first_type_swaps_the_self_slot() {
        let sig = callable(vec![any(EXPLICIT), instance("builtins.int", vec![])]);
        let new = any(SPECIAL_FORM);
        let replaced = replace_implicit_first_type(sig, &new).unwrap();
        let args = arg_types(&replaced);
        assert_eq!(args.len(), 2);
        assert_eq!(any_kind(&args[0]), SPECIAL_FORM);
        assert_eq!(args[1], instance("builtins.int", vec![]));
    }

    #[test]
    fn replace_implicit_first_type_rejects_a_non_callable() {
        let sig = instance("builtins.int", vec![]);
        let new = any(SPECIAL_FORM);
        assert_eq!(replace_implicit_first_type(sig, &new), None);
    }

    #[test]
    fn has_explicit_any_finds_a_nested_explicit_any() {
        let t = instance("builtins.list", vec![any(EXPLICIT)]);
        assert_eq!(has_explicit_any(&t), Some(true));
    }

    #[test]
    fn has_explicit_any_rejects_a_different_any_kind() {
        let t = instance("builtins.list", vec![any(FROM_ERROR)]);
        assert_eq!(has_explicit_any(&t), Some(false));
    }

    #[test]
    fn has_explicit_any_defers_on_an_alias() {
        let t = instance("builtins.list", vec![alias("mod.A")]);
        assert_eq!(has_explicit_any(&t), None);
    }

    #[test]
    fn has_any_from_unimported_type_matches_only_its_own_kind() {
        let unimported = instance("builtins.list", vec![any(FROM_UNIMPORTED_TYPE)]);
        assert_eq!(has_any_from_unimported_type(&unimported), Some(true));
        let explicit = instance("builtins.list", vec![any(EXPLICIT)]);
        assert_eq!(has_any_from_unimported_type(&explicit), Some(false));
    }

    #[test]
    fn check_for_explicit_any_reports_only_when_the_gate_is_open() {
        let t = instance("builtins.list", vec![any(EXPLICIT)]);
        assert_eq!(check_for_explicit_any(&t, true, false), Some(true));
        assert_eq!(check_for_explicit_any(&t, false, false), Some(false));
        assert_eq!(check_for_explicit_any(&t, true, true), Some(false));
    }

    #[test]
    fn check_for_explicit_any_defers_when_the_gate_is_open_on_an_alias() {
        let t = alias("mod.A");
        assert_eq!(check_for_explicit_any(&t, true, false), None);
    }

    #[test]
    fn collect_all_inner_types_returns_children_not_the_root() {
        let inner = any(SPECIAL_FORM);
        let t = instance("builtins.list", vec![inner.clone()]);
        assert_eq!(collect_all_inner_types(&t), Some(vec![inner]));
    }

    #[test]
    fn collect_all_inner_types_defers_on_an_alias() {
        assert_eq!(collect_all_inner_types(&alias("mod.A")), None);
    }

    #[test]
    fn unknown_unpack_accepts_a_special_form_any_target() {
        let t = unpack(any(SPECIAL_FORM));
        assert_eq!(unknown_unpack(&t), Some(true));
    }

    #[test]
    fn unknown_unpack_rejects_a_non_unpack_and_an_aliased_target() {
        assert_eq!(unknown_unpack(&Type::NoneType), Some(false));
        assert_eq!(unknown_unpack(&unpack(any(EXPLICIT))), Some(false));
        assert_eq!(unknown_unpack(&unpack(alias("mod.A"))), None);
    }

    #[test]
    fn is_self_type_fullname_accepts_both_self_spellings() {
        assert!(is_self_type_fullname("typing.Self"));
        assert!(is_self_type_fullname("typing_extensions.Self"));
    }

    #[test]
    fn is_self_type_fullname_rejects_a_plain_name() {
        assert!(!is_self_type_fullname("typing.Any"));
        assert!(!is_self_type_fullname("Self"));
    }

    #[test]
    fn wrong_type_arg_count_msg_renders_the_fixed_arity_forms() {
        let zero = wrong_type_arg_count_msg(0, 0, 0, "Box");
        assert_eq!(zero, "\"Box\" expects no type arguments, but none given");
        let one = wrong_type_arg_count_msg(1, 1, 2, "Box");
        assert_eq!(one, "\"Box\" expects 1 type argument, but 2 given");
    }

    #[test]
    fn wrong_type_arg_count_msg_renders_the_range_form() {
        let ranged = wrong_type_arg_count_msg(1, 3, 4, "Box");
        let expected = "\"Box\" expects between 1 and 3 type arguments, but 4 given";
        assert_eq!(ranged, expected);
    }

    #[test]
    fn special_function_elide_names_accepts_a_positional_only_magic() {
        assert!(special_function_elide_names("__len__"));
        assert!(special_function_elide_names("__add__"));
    }

    #[test]
    fn special_function_elide_names_rejects_a_kwargs_magic() {
        assert!(!special_function_elide_names("__init__"));
        assert!(!special_function_elide_names("__call__"));
        assert!(!special_function_elide_names("regular"));
    }

    #[test]
    fn argument_elide_name_accepts_a_leading_dunder() {
        assert!(argument_elide_name(Some("__x")));
    }

    #[test]
    fn argument_elide_name_rejects_a_dunder_and_an_unnamed_arg() {
        assert!(!argument_elide_name(Some("__x__")));
        assert!(!argument_elide_name(Some("x")));
        assert!(!argument_elide_name(None));
    }

    #[test]
    fn lookup_typeinfo_member_walks_the_mro() {
        let resolver = mro_resolver();
        let found = lookup_typeinfo_member(&resolver, "mod.Sub", "base_method");
        assert_eq!(found, LookupOutcome::Resolved("mod.Base".to_string()));
    }

    #[test]
    fn lookup_typeinfo_member_rejects_an_absent_member() {
        let resolver = mro_resolver();
        let missing = lookup_typeinfo_member(&resolver, "mod.Sub", "nope");
        assert_eq!(missing, LookupOutcome::NotFound);
    }

    #[test]
    fn lookup_typeinfo_member_defers_without_a_snapshot() {
        let resolver = mro_resolver();
        let unknown = lookup_typeinfo_member(&resolver, "mod.Absent", "base_method");
        assert_eq!(unknown, LookupOutcome::Deferred);
    }

    #[test]
    fn lookup_module_chain_resolves_the_owning_module() {
        let mut resolver = TypeResolver::new();
        resolver.insert_module("pkg".to_string(), module(&[("x", false, None)]));
        let found = lookup_module_chain(&resolver, "pkg", "pkg.x");
        assert_eq!(found, LookupOutcome::Resolved("pkg".to_string()));
    }

    #[test]
    fn lookup_module_chain_rejects_a_hidden_name() {
        let mut resolver = TypeResolver::new();
        resolver.insert_module("pkg".to_string(), module(&[("_x", true, None)]));
        assert_eq!(
            lookup_module_chain(&resolver, "pkg", "pkg._x"),
            LookupOutcome::NotFound
        );
    }

    #[test]
    fn lookup_module_chain_defers_on_an_unrecorded_module() {
        let resolver = TypeResolver::new();
        assert_eq!(
            lookup_module_chain(&resolver, "pkg", "pkg.x"),
            LookupOutcome::Deferred
        );
    }
}
