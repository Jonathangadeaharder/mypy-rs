//! Standalone-path public API: types.
//!
//! The type algebra: unions, join, meet, type ops, alias expansion,
//! subtype exposure.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of setops.rs, meet.rs, typeops.rs, types_impl.rs, aliases.rs,
//! builtin_item.rs, subtypes.rs rather than reimplementing them; where a
//! helper needs a Python-side callback today, take the callback as a Rust
//! trait object or an explicit record and say so in the doc comment.
//!
//! Owned exclusively by the `types` lane for this wave. Add
//! `#[cfg(test)]` unit tests here: they keep the lifted API honest and
//! are the only thing that exercises it before the driver integration
//! wave.
//!
//! # Relation to `crate::skeleton_api`
//!
//! `skeleton_api` is the surface the skeleton already checks through:
//! `is_subtype`, `SubtypeContext`, `TypeResolver`, `Type` and the wire
//! writer. The three `pub use` lines below name the same items so this
//! area's surface is self-contained; they re-export, they do not
//! reimplement. Every body here is a call into the module that already
//! owns the ported decision (`crate::setops`, `crate::meet`,
//! `crate::subtypes`, `crate::types_impl`) plus the materialization step
//! the hybrid leaves to its Python shim. Callers reach the kernel
//! functions by full path so the provenance of each lift stays visible at
//! the call site.
//!
//! The `crate::wire` re-exports also carry the value types a `Type`
//! variant holds but `skeleton_api` does not expose: `LiteralValue`,
//! `ExtraAttrs` and `Parameters`. Without them a caller outside this crate
//! can build an `Instance` but not a `LiteralType`, an `Instance` with
//! synthesized attributes, or a `Parameters`, so the algebra it can reach
//! is narrower than the one it can call.
//!
//! # `None` is a decline, never an answer
//!
//! The kernel ports mypy's type algebra strangler-fig style: `Some(v)` is
//! the answer mypy gives, `None` means the snapshot facts on hand cannot
//! reproduce it. A standalone caller has no Python to fall back to, so it
//! must treat `None` as an unsupported construct and reject loudly
//! (wave-1 rule 6). Nothing here converts a decline into a guess, and no
//! wrapper below widens the set of cases the kernel decides.
//!
//! # Alias operands decline until a store can be installed
//!
//! The alias fact store is public and constructible from outside this
//! crate: `TypeAliasResolver`, `TypeAliasSnapshot` and `AliasTvar` are
//! re-exported below, because `mod aliases` is crate-private in `lib.rs`
//! and a `pub` item in a private module is not reachable. Kernel entries
//! that take a `&TypeAliasResolver` directly are therefore usable by a
//! standalone caller today.
//!
//! What is still missing is installation: `TypeResolver::install_aliases`
//! is crate-private on `typeinfo.rs`, so a caller cannot attach a store to
//! the resolver the set operations read. Every operation below that meets
//! a `Type::TypeAliasType` consequently declines, exactly as the hybrid
//! does when no alias view is installed. Wave 2 lifts that once
//! `standalone::records` owns a Python-free alias producer.

pub use crate::aliases::{AliasTvar, TypeAliasResolver, TypeAliasSnapshot};
pub use crate::subtypes::{is_subtype, SubtypeContext};
pub use crate::typeinfo::TypeResolver;
pub use crate::wire::{ExtraAttrs, LiteralValue, Parameters, Type};

/// `UnionType.make_union` (mypy/types.py:3483-3489).
///
/// No items is the empty union (`UninhabitedType`), one item is that item
/// unwrapped, more than one is a `UnionType` whose `can_be_true` and
/// `can_be_false` are the OR over the items. This performs no
/// simplification; [`make_simplified_union`] is mypy's simplifying
/// constructor.
///
/// Lifts `crate::setops::union_make_union`.
pub fn make_union(items: Vec<Type>) -> Type {
    crate::setops::union_make_union(items)
}

/// `make_simplified_union` (mypy/typeops.py:605-692) at mypy's default
/// flags: `contract_literals=True`, `keep_erased=False`,
/// `handle_recursive=True`.
///
/// Flattens nested unions, unwraps the single-item case, drops every item
/// another item already covers, contracts a bool or enum literal set whose
/// values are fully covered back to its fallback instance, then rebuilds
/// with [`make_union`].
///
/// Declines when a nested item is a `TypeAliasType` (no standalone alias
/// store), when a redundancy test needs a subtype answer the snapshots
/// cannot give, or when literal contraction needs an enum the resolver
/// does not carry.
///
/// Lifts `crate::setops::make_simplified_union`.
pub fn make_simplified_union(
    items: &[Type],
    ctx: &SubtypeContext,
    resolver: &TypeResolver,
) -> Option<Type> {
    crate::setops::make_simplified_union(items, ctx, resolver, true, false)
}

/// `flatten_nested_unions` (mypy/types.py:4267-4300): expand nested
/// `UnionType` items into one flat list.
///
/// Declines on a `TypeAliasType` item: the wire form carries the alias
/// name only, so flattening it needs the alias target.
///
/// Lifts `crate::setops::flatten_nested_unions`.
pub fn flatten_nested_unions(items: &[Type]) -> Option<Vec<Type>> {
    crate::setops::flatten_nested_unions(items)
}

/// `UnionType.length` (mypy/types.py:3511-3512): the item count, or `None`
/// for a type that is not a union.
///
/// Lifts `crate::types_impl::union_length_inner`.
pub fn union_length(t: &Type) -> Option<i64> {
    crate::types_impl::union_length_inner(t)
}

/// `join_types` (mypy/join.py:294-330), materialized.
///
/// The least upper bound of `s` and `t`: the wider operand when one
/// subsumes the other, the nearest common ancestor for two instances,
/// `builtins.object` when nothing closer exists, a merged union for a
/// union operand, and the per-family result for callables, tuples, typed
/// dicts, literals and type variables.
///
/// The hybrid seam (`crate::setops::rust_join_types_inner`) runs the same
/// three steps but builds its own `SubtypeContext` from a bare
/// `strict_optional` flag. This entry takes the caller's context instead,
/// so a standalone driver keeps one context for its whole pass.
/// Materialization is `crate::setops::materialize_join`, the kernel's own
/// full converter for a join result.
///
/// Declines when a nested case needs facts the snapshots do not carry, or
/// when an operand is a `TypeAliasType` and no alias view is installed.
pub fn join_types(
    s: &Type,
    t: &Type,
    ctx: &SubtypeContext,
    resolver: &TypeResolver,
) -> Option<Type> {
    let (s, t) = crate::setops::expand_top_alias_pair(s, t, resolver)?;
    let fruit = crate::setops::join_types(&s, &t, ctx, resolver)?;
    crate::setops::materialize_join(&s, &t, fruit, resolver)
}

/// `trivial_join` (mypy/join.py:198-205): the subtype-only join.
///
/// Returns the wider operand when one is a subtype of the other, otherwise
/// `builtins.object` (mypy's `object_or_any_from_type` on an instance). A
/// pair of function-like operands declines, because mypy joins those
/// structurally rather than by subsumption.
///
/// Lifts `crate::setops::trivial_join`, materialized with
/// `crate::setops::fruit_to_type`.
pub fn trivial_join(
    s: &Type,
    t: &Type,
    ctx: &SubtypeContext,
    resolver: &TypeResolver,
) -> Option<Type> {
    let fruit = crate::setops::trivial_join(s, t, ctx, resolver)?;
    crate::setops::fruit_to_type(fruit, s, t)
}

/// `meet_types` (mypy/meet.py:118-206), materialized.
///
/// The greatest lower bound of `s` and `t`: the narrower operand when one
/// is a proper subtype of the other, the per-item meet for two unions, the
/// structural meet for two callables, and `bottom_type` for disjoint
/// operands.
///
/// Materialization is `crate::setops::fruit_to_type`, the converter the
/// kernel itself uses for per-argument meets, rather than the hybrid
/// seam's `crate::setops::setop_result_to_type`. That one declines on the
/// `Encoded` and `SameTypeWithArgs` results and filters every answer
/// through a wire-encodability test; both exist only because the seam's
/// answer has to cross back into Python, and a standalone caller makes no
/// such crossing. The `Bottom` variant is remapped through `bottom_type`
/// because its shape depends on `strict_optional`, which `fruit_to_type`
/// does not take.
///
/// Declines for the same reasons [`join_types`] does.
pub fn meet_types(
    s: &Type,
    t: &Type,
    ctx: &SubtypeContext,
    resolver: &TypeResolver,
) -> Option<Type> {
    let (s, t) = crate::setops::expand_top_alias_pair(s, t, resolver)?;
    let fruit = crate::setops::meet_types(&s, &t, ctx, resolver)?;
    meet_fruit(fruit, &s, &t, ctx.strict_optional)
}

/// `trivial_meet` (mypy/meet.py:62-72): the subtype-only meet.
///
/// Returns the narrower operand when one is a subtype of the other,
/// otherwise the empty meet (`bottom_type`). A pair of function-like
/// operands declines, as in [`trivial_join`].
///
/// Lifts `crate::setops::trivial_meet`.
pub fn trivial_meet(
    s: &Type,
    t: &Type,
    ctx: &SubtypeContext,
    resolver: &TypeResolver,
) -> Option<Type> {
    let fruit = crate::setops::trivial_meet(s, t, ctx, resolver)?;
    meet_fruit(fruit, s, t, ctx.strict_optional)
}

/// `narrow_declared_type` (mypy/meet.py:216-348): narrow a declared type
/// by a narrower one, the operation behind every `isinstance` and
/// comparison narrowing.
///
/// A declared union narrows per item and is re-simplified; a declared type
/// variable narrows within its upper bound; a callable narrows through its
/// return type; disjoint operands give the empty meet. Declines when a case
/// needs a live `TypeInfo` outside the snapshots (a `TypedDictType`, say),
/// when an operand is an unexpandable `TypeAliasType`, or when the pair is
/// one the kernel refuses to guess about.
///
/// Lifts `crate::meet::narrow_rec` with no alias view, which is what the
/// hybrid seam passes for a resolver carrying no alias snapshots.
pub fn narrow_declared_type(
    declared: &Type,
    narrowed: &Type,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Type> {
    crate::meet::narrow_rec(declared, narrowed, strict_optional, None, resolver)
}

/// `is_overlapping_types` (mypy/meet.py:450-774): whether two types can
/// hold the same value. The parameter order mirrors mypy's.
///
/// Declines when a case needs a live `TypeInfo`, a protocol member walk
/// the snapshots cannot back, or an alias operand with no installed alias
/// view.
///
/// Lifts `crate::meet::overlap` at depth 0. The kernel caps its own
/// recursion depth where mypy carries a `seen_types` guard, which cannot
/// cross a function boundary that takes no state.
pub fn is_overlapping_types(
    left: &Type,
    right: &Type,
    ignore_promotions: bool,
    overlap_for_overloads: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::meet::overlap(
        left,
        right,
        strict_optional,
        ignore_promotions,
        overlap_for_overloads,
        resolver,
        0,
    )
}

/// `is_tuple` (mypy/typeops.py:632-635): a `TupleType`, or an `Instance`
/// of `builtins.tuple`.
///
/// Lifts `crate::meet::is_tuple`.
pub fn is_tuple(t: &Type) -> bool {
    crate::meet::is_tuple(t)
}

/// `is_same_type` (mypy/subtypes.py:1063-1090).
///
/// Lifts `crate::subtypes::is_same_type`.
pub fn is_same_type(
    a: &Type,
    b: &Type,
    ignore_promotions: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::subtypes::is_same_type(a, b, ignore_promotions, strict_optional, resolver)
}

/// `is_equivalent` (mypy/subtypes.py:277-300): each side is a subtype of
/// the other.
///
/// Lifts `crate::subtypes::is_equivalent`.
pub fn is_equivalent(
    a: &Type,
    b: &Type,
    ignore_type_params: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::subtypes::is_equivalent(a, b, ignore_type_params, strict_optional, resolver)
}

/// `is_more_precise` (mypy/subtypes.py:2895-2905): `left` is a proper
/// subtype of `right`, or `right` is `Any`.
///
/// Lifts `crate::subtypes::is_more_precise`.
pub fn is_more_precise(
    left: &Type,
    right: &Type,
    ignore_promotions: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    crate::subtypes::is_more_precise(left, right, ignore_promotions, strict_optional, resolver)
}

/// `map_instance_to_supertype` (mypy/maptype.py:8-23): the type arguments
/// `left_ref[left_args]` presents to one of its supertypes.
///
/// Declines when neither side has a snapshot and no live `TypeInfo` map is
/// installed, or when a derivation step hits a type the expander does not
/// carry (a variadic left, for one).
///
/// Lifts `crate::subtypes::map_instance_to_supertype`.
pub fn map_instance_to_supertype(
    left_ref: &str,
    left_args: &[Type],
    right_ref: &str,
    resolver: &TypeResolver,
) -> Option<Vec<Type>> {
    crate::subtypes::map_instance_to_supertype(left_ref, left_args, right_ref, resolver)
}

/// `is_named_instance` (mypy/typeops.py:636-640): an `Instance` of exactly
/// `fullname`. Never declines; it is a shape test, not a semantic one.
///
/// Lifts `crate::subtypes::is_named_instance`.
pub fn is_named_instance(t: &Type, fullname: &str) -> bool {
    crate::subtypes::is_named_instance(t, fullname)
}

/// `get_proper_type` (mypy/types.py): expand a top-level `TypeAliasType`
/// to its substituted target. Every other variant is already proper and is
/// returned unchanged.
///
/// Expansion reads the resolver's installed alias view, so this declines
/// exactly where the standalone alias store is missing (module docs).
///
/// Lifts `crate::checkexpr_functions::proper_or_expand_resolver`.
pub fn get_proper_type(t: &Type, resolver: &TypeResolver) -> Option<Type> {
    crate::checkexpr_functions::proper_or_expand_resolver(t, resolver)
}

/// `Type.can_be_true_default` (mypy/types.py:295-3459): whether a value of
/// `t` can be truthy, from the per-variant defaults alone.
///
/// Declines where the default depends on live Python state: a
/// `TypeAliasType` (needs `alias.target`), a `TupleType` (needs
/// `can_be_any_bool`) and a non-bool `LiteralType` over an `Instance`
/// fallback (needs `TypeInfo.is_enum`).
///
/// Lifts `crate::typeops::can_be_true_default`.
pub fn can_be_true_default(t: &Type) -> Option<bool> {
    crate::typeops::can_be_true_default(t)
}

/// `Type.can_be_false_default` (mypy/types.py:298-3459): whether a value
/// of `t` can be falsy, from the per-variant defaults alone. Declines for
/// the same variants as [`can_be_true_default`].
///
/// Lifts `crate::typeops::can_be_false_default`.
pub fn can_be_false_default(t: &Type) -> Option<bool> {
    crate::typeops::can_be_false_default(t)
}

/// `is_simple_literal` (mypy/typeops.py:597-602): a `LiteralType` whose
/// fallback is an enum or `builtins.str`, or an `Instance` whose
/// `last_known_value` is a string literal.
///
/// Declines when an enum check needs a snapshot the resolver does not
/// carry.
///
/// Lifts `crate::typeops::is_simple_literal`.
pub fn is_simple_literal(t: &Type, resolver: &TypeResolver) -> Option<bool> {
    crate::typeops::is_simple_literal(t, resolver)
}

/// `erase_to_bound` (mypy/typeops.py:629-637): replace a type variable
/// with its upper bound. A `TypeVarType` yields its `upper_bound`, a
/// `TypeType` over one yields `TypeType(upper_bound)`, everything else
/// passes through. Declines on a `TypeAliasType`.
///
/// Lifts `crate::typeops::erase_to_bound`.
pub fn erase_to_bound(t: &Type) -> Option<Type> {
    crate::typeops::erase_to_bound(t)
}

/// `tuple_fallback` (mypy/typeops.py:194-220): the `Instance` a
/// `TupleType` falls back to. A partial fallback that is not
/// `builtins.tuple` is returned as is; otherwise the items are collected
/// into `Instance(builtins.tuple, [make_simplified_union(items)])`.
///
/// Declines for a non-`TupleType`, for an `UnpackType` unpacking to a
/// non-tuple (where mypy raises `NotImplementedError`), and when the item
/// union cannot be simplified.
///
/// Lifts `crate::typeops::tuple_fallback`.
pub fn tuple_fallback(t: &Type, resolver: &TypeResolver) -> Option<Type> {
    crate::typeops::tuple_fallback(t, resolver)
}

/// `try_expanding_sum_type_to_union` (mypy/typeops.py:1292-1333): expand a
/// sum type into the union of its members. `builtins.bool` becomes
/// `Union[Literal[True], Literal[False]]`, a union expands per item, and
/// anything else passes through. `target_fullname` restricts the expansion
/// to one class, as mypy's narrowing callers do.
///
/// Declines on a `TypeAliasType` and on an enum, whose `enum_members`
/// snapshot can be stale where mypy reads them live.
///
/// Lifts `crate::typeops::try_expanding_sum_type_to_union_inner`.
pub fn try_expanding_sum_type_to_union(
    t: &Type,
    target_fullname: Option<&str>,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Type> {
    crate::typeops::try_expanding_sum_type_to_union_inner(
        t,
        target_fullname,
        strict_optional,
        resolver,
    )
}

/// `get_type_vars` (mypy/typeops.py:1449-1450), or `get_all_type_vars`
/// when `include_all` is set: every type variable reachable in `t`.
///
/// Declines on any `TypeAliasType`, because this traversal takes no alias
/// view; the hybrid's resolver-backed seam does.
///
/// Lifts `crate::typeops::collect_type_vars`, wrapping its out-parameter
/// shape in the returned list a caller expects.
pub fn get_type_vars(t: &Type, include_all: bool) -> Option<Vec<Type>> {
    let mut out = Vec::new();
    crate::typeops::collect_type_vars(t, include_all, None, &mut Vec::new(), &mut out)?;
    Some(out)
}

/// `is_recursive_pair` (mypy/typeops.py:344-362): whether a pair of types
/// can re-enter the same subtype or set-operation recursion. This is the
/// guard mypy's public entries run before walking.
///
/// Without an alias view only the resolver-free branches decide: both
/// operands recursive aliases is `true`, neither recursive is `false`, and
/// a mixed pair declines.
///
/// Lifts `crate::typeops::recursive_pair_core`.
pub fn is_recursive_pair(s: &Type, t: &Type) -> Option<bool> {
    crate::typeops::recursive_pair_core(s, t, None)
}

/// `bind_self` (mypy/typeops.py:540-641): strip the first parameter of a
/// method's `CallableType` and mark it bound, the shape a member access
/// hands to the call checker.
///
/// Declines for a non-callable, for a callable with no parameters, for one
/// whose first parameter is `*args` or `**kwargs`, and for a generic
/// callable, which needs `infer_type_arguments`.
///
/// Lifts `crate::typeops::bind_self_inner`.
pub fn bind_self(t: &Type) -> Option<Type> {
    crate::typeops::bind_self_inner(t)
}

/// `CallableType.min_args` (mypy/types.py:2330-2331): the count of
/// required positional parameters. `None` for a non-callable.
///
/// Lifts `crate::types_impl::callable_min_args_inner`.
pub fn callable_min_args(t: &Type) -> Option<i64> {
    crate::types_impl::callable_min_args_inner(t)
}

/// `CallableType.is_var_arg` (mypy/types.py:2334-2336): does the callable
/// take `*args`. `None` for a non-callable.
///
/// Lifts `crate::types_impl::callable_is_var_arg_inner`.
pub fn callable_is_var_arg(t: &Type) -> Option<bool> {
    crate::types_impl::callable_is_var_arg_inner(t)
}

/// `CallableType.is_kw_arg` (mypy/types.py:2339-2341): does the callable
/// take `**kwargs`. `None` for a non-callable.
///
/// Lifts `crate::types_impl::callable_is_kw_arg_inner`.
pub fn callable_is_kw_arg(t: &Type) -> Option<bool> {
    crate::types_impl::callable_is_kw_arg_inner(t)
}

/// `CallableType.max_possible_positional_args` (mypy/types.py:2390-2396):
/// the positional count, or `i64::MAX` when `*args` or `**kwargs` makes it
/// unbounded. `None` for a non-callable.
///
/// Lifts `crate::types_impl::callable_max_possible_positional_args_inner`.
pub fn callable_max_possible_positional_args(t: &Type) -> Option<i64> {
    crate::types_impl::callable_max_possible_positional_args_inner(t)
}

/// `CallableType.is_generic` (mypy/types.py:2471-2472): does the callable
/// declare type variables. `None` for a non-callable.
///
/// Lifts `crate::types_impl::callable_is_generic_inner`.
pub fn callable_is_generic(t: &Type) -> Option<bool> {
    crate::types_impl::callable_is_generic_inner(t)
}

/// `TupleType.length` (mypy/types.py:2896-2897): the item count. `None`
/// for a non-tuple.
///
/// Lifts `crate::types_impl::tuple_length_inner`.
pub fn tuple_length(t: &Type) -> Option<i64> {
    crate::types_impl::tuple_length_inner(t)
}

/// Materialize a meet result. Every variant goes to
/// `crate::setops::fruit_to_type` except `Bottom`, whose mapping depends
/// on `strict_optional` and which `fruit_to_type` therefore pins at the
/// strict shape. The branch is the kernel's own:
/// `crate::meet::materialize_meet_result` maps `Bottom` the same way for
/// `narrow_declared_type`.
fn meet_fruit(
    fruit: crate::setops::SetOpResult,
    s: &Type,
    t: &Type,
    strict_optional: bool,
) -> Option<Type> {
    match fruit {
        crate::setops::SetOpResult::Bottom => Some(bottom_type(strict_optional)),
        other => crate::setops::fruit_to_type(other, s, t),
    }
}

/// The empty type of a meet: `UninhabitedType` under strict optional and
/// `NoneType` without it (mypy/meet.py:1541-1548,
/// `TypeMeetVisitor.default`).
fn bottom_type(strict_optional: bool) -> Type {
    if strict_optional {
        Type::UninhabitedType { ambiguous: false }
    } else {
        Type::NoneType
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typeinfo::TypeInfoSnapshot;

    fn ctx(strict_optional: bool) -> SubtypeContext {
        SubtypeContext {
            strict_optional,
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

    fn any_type() -> Type {
        Type::AnyType {
            type_of_any: 0,
            source_any: None,
            missing_import_name: None,
        }
    }

    fn uninhabited() -> Type {
        Type::UninhabitedType { ambiguous: false }
    }

    /// One class snapshot: `mro` and `has_base` carry the class itself
    /// plus the implicit `builtins.object`.
    fn snap(fullname: &str) -> TypeInfoSnapshot {
        let mut s = TypeInfoSnapshot {
            fullname: fullname.to_string(),
            name: fullname.to_string(),
            ..Default::default()
        };
        s.mro.push(fullname.to_string());
        s.has_base.insert(fullname.to_string());
        if fullname != "builtins.object" {
            s.mro.push("builtins.object".to_string());
            s.has_base.insert("builtins.object".to_string());
        }
        s
    }

    /// A class snapshot declaring `base_refs` as its bases, each encoded
    /// the way the kernel reads `TypeInfoSnapshot::bases`.
    fn snap_with_bases(fullname: &str, base_refs: &[&str]) -> TypeInfoSnapshot {
        let mut s = snap(fullname);
        let mut bases = Vec::new();
        for base_ref in base_refs {
            bases.push(crate::wire::encode_instance_simple_for_test(base_ref));
            s.has_base.insert((*base_ref).to_string());
            s.mro.push((*base_ref).to_string());
        }
        s.bases = bases;
        s
    }

    fn resolver(snaps: Vec<TypeInfoSnapshot>) -> TypeResolver {
        let mut r = TypeResolver::new();
        for s in snaps {
            r.insert(s.fullname.clone(), s);
        }
        r
    }

    /// The class facts most decisions below are made against: `a.A`, a
    /// subclass `a.B`, and an unrelated `a.Z`.
    fn hierarchy() -> TypeResolver {
        let base = snap("a.A");
        let derived = snap_with_bases("a.B", &["a.A"]);
        let other = snap("a.Z");
        resolver(vec![base, derived, other])
    }

    /// The three stdlib classes the narrowing and overlap tests need.
    fn builtins_resolver() -> TypeResolver {
        let int = snap("builtins.int");
        let text = snap("builtins.str");
        let obj = snap("builtins.object");
        resolver(vec![int, text, obj])
    }

    #[test]
    fn make_union_follows_the_item_count() {
        // types.py:3483-3489: no items is bottom, one item unwraps.
        let empty: Vec<Type> = Vec::new();
        assert_eq!(make_union(empty), uninhabited());
        let a = instance("a.A");
        assert_eq!(make_union(vec![a.clone()]), a);
        let two = make_union(vec![instance("a.A"), instance("a.B")]);
        assert_eq!(union_length(&two), Some(2));
    }

    #[test]
    fn union_length_declines_on_a_non_union() {
        let a = instance("a.A");
        assert_eq!(union_length(&a), None);
    }

    #[test]
    fn make_simplified_union_drops_an_item_another_covers() {
        // typeops.py:695-771: a.B <: a.A, so the union is just a.A.
        let r = hierarchy();
        let items = [instance("a.A"), instance("a.B")];
        let a = instance("a.A");
        let out = make_simplified_union(&items, &ctx(true), &r);
        assert_eq!(out, Some(a));
    }

    #[test]
    fn make_simplified_union_declines_on_an_alias_item() {
        // types.py:4267: flattening an alias needs its target, and no
        // alias view is installed, so the honest answer is a decline.
        let r = hierarchy();
        let items = [alias_ref("mod.A"), instance("a.A")];
        let out = make_simplified_union(&items, &ctx(true), &r);
        assert_eq!(out, None);
    }

    #[test]
    fn flatten_nested_unions_flattens_and_declines() {
        let inner = make_union(vec![instance("a.A"), instance("a.B")]);
        let outer = make_union(vec![inner, instance("a.C")]);
        let flat = flatten_nested_unions(&[outer]);
        assert_eq!(flat.map(|items| items.len()), Some(3));
        let aliased = [alias_ref("mod.A")];
        assert_eq!(flatten_nested_unions(&aliased), None);
    }

    #[test]
    fn join_of_two_instances_gives_their_common_base() {
        // join.py:294 into the nominal instance join: a.D and a.E both
        // derive from a.C and neither subsumes the other.
        let c = snap("a.C");
        let d = snap_with_bases("a.D", &["a.C"]);
        let e = snap_with_bases("a.E", &["a.C"]);
        let r = resolver(vec![c, d, e]);
        let left = instance("a.D");
        let right = instance("a.E");
        let out = join_types(&left, &right, &ctx(true), &r);
        assert_eq!(out, Some(instance("a.C")));
    }

    #[test]
    fn join_declines_on_an_alias_operand() {
        let r = hierarchy();
        let alias = alias_ref("mod.A");
        let a = instance("a.A");
        let out = join_types(&alias, &a, &ctx(true), &r);
        assert_eq!(out, None);
    }

    #[test]
    fn trivial_join_picks_the_wider_operand_then_object() {
        // join.py:198-205: a.B <: a.A gives a.A; two unrelated classes
        // fall back to builtins.object.
        let r = hierarchy();
        let a = instance("a.A");
        let b = instance("a.B");
        let out = trivial_join(&a, &b, &ctx(true), &r);
        assert_eq!(out, Some(instance("a.A")));
        let z = instance("a.Z");
        let out = trivial_join(&a, &z, &ctx(true), &r);
        assert_eq!(out, Some(instance("builtins.object")));
    }

    #[test]
    fn meet_of_a_class_and_its_subclass_narrows_to_the_subclass() {
        // meet.py:139-141: a.B is a proper subtype of a.A, so the meet
        // is the narrower operand.
        let r = hierarchy();
        let a = instance("a.A");
        let b = instance("a.B");
        let out = meet_types(&a, &b, &ctx(true), &r);
        assert_eq!(out, Some(b));
    }

    #[test]
    fn meet_declines_on_an_alias_operand() {
        let r = hierarchy();
        let alias = alias_ref("mod.A");
        let a = instance("a.A");
        let out = meet_types(&alias, &a, &ctx(true), &r);
        assert_eq!(out, None);
    }

    #[test]
    fn trivial_meet_bottom_follows_strict_optional() {
        // meet.py:62-72 with TypeMeetVisitor.default (meet.py:1541-1548):
        // disjoint operands meet at UninhabitedType, or at NoneType once
        // strict optional is off. This is the control for `bottom_type`.
        let r = hierarchy();
        let a = instance("a.A");
        let z = instance("a.Z");
        let strict = trivial_meet(&a, &z, &ctx(true), &r);
        assert_eq!(strict, Some(uninhabited()));
        let loose = trivial_meet(&a, &z, &ctx(false), &r);
        assert_eq!(loose, Some(Type::NoneType));
    }

    #[test]
    fn trivial_meet_returns_the_narrower_operand() {
        let r = hierarchy();
        let a = instance("a.A");
        let b = instance("a.B");
        let out = trivial_meet(&a, &b, &ctx(true), &r);
        assert_eq!(out, Some(b));
    }

    #[test]
    fn narrow_declared_type_narrows_a_union_per_item() {
        // meet.py:225-244: Union[int, str] narrowed by int keeps the
        // overlapping item and drops the disjoint one.
        let r = builtins_resolver();
        let int = instance("builtins.int");
        let text = instance("builtins.str");
        let declared = make_union(vec![int.clone(), text]);
        let out = narrow_declared_type(&declared, &int, true, &r);
        assert_eq!(out, Some(int));
    }

    #[test]
    fn narrow_declared_type_makes_disjoint_operands_empty() {
        // meet.py:271-276: disjoint operands under strict optional.
        let r = builtins_resolver();
        let int = instance("builtins.int");
        let text = instance("builtins.str");
        let out = narrow_declared_type(&int, &text, true, &r);
        assert_eq!(out, Some(uninhabited()));
    }

    #[test]
    fn narrow_declared_type_declines_on_an_undecidable_pair() {
        // The declared == narrowed identity branch (meet.py:224) belongs
        // to the Python shim, so for a class the resolver does not carry
        // the overlap probe has no facts and must decline.
        let r = resolver(Vec::new());
        let unknown = instance("a.Unknown");
        let out = narrow_declared_type(&unknown, &unknown, true, &r);
        assert_eq!(out, None);
    }

    #[test]
    fn is_overlapping_types_decides_a_repeated_instance() {
        let r = builtins_resolver();
        let int = instance("builtins.int");
        let out = is_overlapping_types(&int, &int, false, false, true, &r);
        assert_eq!(out, Some(true));
    }

    #[test]
    fn is_overlapping_types_declines_on_an_alias_operand() {
        // meet.py:556 runs get_proper_type first; an alias with no
        // installed view cannot be expanded, so the answer is a decline.
        let r = builtins_resolver();
        let alias = alias_ref("mod.A");
        let int = instance("builtins.int");
        let out = is_overlapping_types(&alias, &int, false, false, true, &r);
        assert_eq!(out, None);
    }

    #[test]
    fn is_tuple_recognizes_both_tuple_shapes() {
        let fallback = instance("builtins.tuple");
        let tuple = Type::TupleType {
            partial_fallback: Box::new(fallback),
            items: vec![instance("builtins.int")],
            implicit: false,
        };
        assert!(is_tuple(&tuple));
        assert!(is_tuple(&instance("builtins.tuple")));
        assert!(!is_tuple(&instance("builtins.list")));
    }

    #[test]
    fn is_same_type_fast_paths_and_declines() {
        let r = hierarchy();
        let a = instance("a.A");
        let b = instance("a.B");
        assert_eq!(is_same_type(&a, &a, false, true, &r), Some(true));
        assert_eq!(is_same_type(&a, &b, false, true, &r), Some(false));
        let alias = alias_ref("mod.A");
        assert_eq!(is_same_type(&alias, &a, false, true, &r), None);
    }

    #[test]
    fn is_equivalent_follows_subtyping_both_ways() {
        let r = hierarchy();
        let a = instance("a.A");
        let b = instance("a.B");
        assert_eq!(is_equivalent(&a, &a, false, true, &r), Some(true));
        assert_eq!(is_equivalent(&a, &b, false, true, &r), Some(false));
        let alias = alias_ref("mod.A");
        assert_eq!(is_equivalent(&alias, &a, false, true, &r), None);
    }

    #[test]
    fn is_more_precise_accepts_any_and_declines_on_an_alias() {
        let r = hierarchy();
        let a = instance("a.A");
        let z = instance("a.Z");
        let any = any_type();
        // subtypes.py:2895-2905: an Any right is always less precise.
        assert_eq!(is_more_precise(&a, &any, false, true, &r), Some(true));
        assert_eq!(is_more_precise(&a, &z, false, true, &r), Some(false));
        let alias = alias_ref("mod.A");
        assert_eq!(is_more_precise(&alias, &a, false, true, &r), None);
    }

    #[test]
    fn map_instance_to_supertype_fast_paths_and_declines() {
        let r = resolver(vec![snap("a.A")]);
        // maptype.py:15-17: a class maps to its own arguments.
        let args = [instance("builtins.int")];
        let mapped = map_instance_to_supertype("a.A", &args, "a.A", &r);
        assert_eq!(mapped, Some(args.to_vec()));
        // maptype.py:8-23 with no snapshot for either side and no live
        // TypeInfo map: there is nothing to derive from.
        let empty = resolver(Vec::new());
        let none = map_instance_to_supertype("a.X", &[], "a.Y", &empty);
        assert_eq!(none, None);
    }

    #[test]
    fn is_named_instance_matches_the_fullname_only() {
        let a = instance("a.A");
        assert!(is_named_instance(&a, "a.A"));
        assert!(!is_named_instance(&a, "a.B"));
        assert!(!is_named_instance(&any_type(), "a.A"));
    }

    #[test]
    fn the_reexported_is_subtype_sees_the_same_facts() {
        // The algebra must agree with the subtype entry `skeleton_api`
        // already exposes, not shadow it with a second decision.
        let r = hierarchy();
        let a = instance("a.A");
        let b = instance("a.B");
        let out = is_subtype(&b, &a, &ctx(true), &r);
        assert_eq!(out, Some(true));
    }

    // The fixtures and tests below cover the `typeops` and `types.py`
    // accessor family.

    /// `mypy.nodes.ArgKind` values as the wire carries them.
    const ARG_POS: i64 = 0;
    const ARG_OPT: i64 = 1;
    const ARG_STAR: i64 = 2;
    const ARG_STAR2: i64 = 4;

    fn type_var(raw_id: i64, name: &str) -> Type {
        Type::TypeVarType {
            name: name.to_string(),
            fullname: format!("mod.{name}"),
            raw_id,
            namespace: String::new(),
            values: Vec::new(),
            upper_bound: Box::new(any_type()),
            default: Box::new(any_type()),
            variance: 0,
            meta_level: 0,
        }
    }

    /// A `CallableType` over `arg_kinds`, each parameter an `object`.
    fn callable(arg_kinds: Vec<i64>, variables: Vec<Type>) -> Type {
        let arg_types = vec![instance("builtins.object"); arg_kinds.len()];
        let arg_names = vec![None; arg_kinds.len()];
        Type::CallableType {
            fallback: Box::new(instance("builtins.function")),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: false,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs: false,
            from_type_type: false,
            arg_types,
            arg_kinds,
            arg_names,
            ret_type: Box::new(instance("builtins.object")),
            name: None,
            variables,
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    fn tuple(items: Vec<Type>) -> Type {
        Type::TupleType {
            partial_fallback: Box::new(instance("builtins.tuple")),
            items,
            implicit: false,
        }
    }

    fn recursive_alias(fullname: &str) -> Type {
        Type::TypeAliasType {
            args: Vec::new(),
            type_ref: fullname.to_string(),
            is_recursive: true,
        }
    }

    fn literal(value: LiteralValue, fallback: &str) -> Type {
        Type::LiteralType {
            fallback: Box::new(instance(fallback)),
            value,
        }
    }

    fn bool_literal(value: bool) -> Type {
        literal(LiteralValue::Bool(value), "builtins.bool")
    }

    fn int_literal(value: i64) -> Type {
        literal(LiteralValue::Int(value), "builtins.int")
    }

    fn str_literal(value: &str) -> Type {
        let text = LiteralValue::Str(value.to_string());
        literal(text, "builtins.str")
    }

    #[test]
    fn get_proper_type_passes_a_proper_type_through() {
        // mypy's get_proper_type: only an alias has a proper form to
        // compute, every other variant is already proper.
        let r = builtins_resolver();
        let int = instance("builtins.int");
        assert_eq!(get_proper_type(&int, &r), Some(int.clone()));
    }

    #[test]
    fn get_proper_type_declines_on_an_alias() {
        let r = builtins_resolver();
        let alias = alias_ref("mod.A");
        assert_eq!(get_proper_type(&alias, &r), None);
    }

    #[test]
    fn truthiness_defaults_follow_the_variant() {
        // types.py:295-3459 per-variant defaults.
        let int = instance("builtins.int");
        assert_eq!(can_be_true_default(&int), Some(true));
        assert_eq!(can_be_false_default(&int), Some(true));
        let never = uninhabited();
        assert_eq!(can_be_true_default(&never), Some(false));
        assert_eq!(can_be_false_default(&never), Some(false));
        assert_eq!(can_be_true_default(&Type::NoneType), Some(false));
        assert_eq!(can_be_false_default(&Type::NoneType), Some(true));
        let yes = bool_literal(true);
        assert_eq!(can_be_true_default(&yes), Some(true));
        assert_eq!(can_be_false_default(&yes), Some(false));
    }

    #[test]
    fn truthiness_defaults_decline_where_a_snapshot_decides() {
        // An int literal's truthiness depends on TypeInfo.is_enum and a
        // tuple's on can_be_any_bool; neither is a per-variant default.
        let plain = int_literal(1);
        assert_eq!(can_be_true_default(&plain), None);
        assert_eq!(can_be_false_default(&plain), None);
        let items = vec![instance("builtins.int")];
        assert_eq!(can_be_true_default(&tuple(items)), None);
    }

    #[test]
    fn is_simple_literal_follows_the_fallback() {
        // typeops.py:597-602: a str literal is simple, an int literal is
        // not, and a plain instance is not either.
        let r = builtins_resolver();
        let text = str_literal("x");
        assert_eq!(is_simple_literal(&text, &r), Some(true));
        let number = int_literal(1);
        assert_eq!(is_simple_literal(&number, &r), Some(false));
        let int = instance("builtins.int");
        assert_eq!(is_simple_literal(&int, &r), Some(false));
        // The enum check needs the fallback snapshot; without it the
        // answer is a decline, not a guessed false.
        let empty = resolver(Vec::new());
        assert_eq!(is_simple_literal(&number, &empty), None);
    }

    #[test]
    fn erase_to_bound_replaces_a_type_variable_with_its_bound() {
        // typeops.py:629-637.
        let tv = type_var(1, "T");
        assert_eq!(erase_to_bound(&tv), Some(any_type()));
        let int = instance("builtins.int");
        assert_eq!(erase_to_bound(&int), Some(int.clone()));
    }

    #[test]
    fn erase_to_bound_declines_on_an_alias() {
        let alias = alias_ref("mod.A");
        assert_eq!(erase_to_bound(&alias), None);
    }

    #[test]
    fn tuple_fallback_builds_the_tuple_instance() {
        // typeops.py:194-220: a builtins.tuple partial fallback collects
        // the items into Instance(builtins.tuple, [union of items]).
        let r = builtins_resolver();
        let int = instance("builtins.int");
        let tup = tuple(vec![int.clone()]);
        let out = tuple_fallback(&tup, &r);
        let Type::Instance { type_ref, args, .. } = out.unwrap() else {
            panic!("expected an Instance fallback");
        };
        assert_eq!(type_ref, "builtins.tuple");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], int);
    }

    #[test]
    fn tuple_fallback_declines_on_a_non_tuple() {
        let r = builtins_resolver();
        let int = instance("builtins.int");
        assert_eq!(tuple_fallback(&int, &r), None);
    }

    #[test]
    fn try_expanding_sum_type_expands_bool_into_literals() {
        // typeops.py:1292-1333: bool is the union of its two literals.
        let r = builtins_resolver();
        let b = instance("builtins.bool");
        let out = try_expanding_sum_type_to_union(&b, None, true, &r);
        let Type::UnionType { items, .. } = out.unwrap() else {
            panic!("expected a union of the two bool literals");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], bool_literal(true));
        assert_eq!(items[1], bool_literal(false));
    }

    #[test]
    fn try_expanding_sum_type_honours_a_target_and_declines_on_an_alias() {
        let r = builtins_resolver();
        let b = instance("builtins.bool");
        // A target that does not match leaves the type unchanged.
        let off_target = Some("builtins.int");
        let out = try_expanding_sum_type_to_union(&b, off_target, true, &r);
        assert_eq!(out, Some(b));
        let alias = alias_ref("mod.A");
        let expanded = try_expanding_sum_type_to_union(&alias, None, true, &r);
        assert_eq!(expanded, None);
    }

    #[test]
    fn get_type_vars_collects_a_nested_type_variable() {
        // typeops.py:1449-1450: instance args are traversed.
        let tv = type_var(1, "T");
        let list_of_t = Type::Instance {
            type_ref: "builtins.list".to_string(),
            args: vec![tv.clone()],
            last_known_value: None,
            extra_attrs: None,
        };
        assert_eq!(get_type_vars(&list_of_t, false), Some(vec![tv]));
    }

    #[test]
    fn get_type_vars_declines_on_an_alias() {
        let alias = alias_ref("mod.A");
        assert_eq!(get_type_vars(&alias, false), None);
    }

    #[test]
    fn is_recursive_pair_decides_only_the_resolver_free_branches() {
        // typeops.py:344-362: both recursive is true, neither is false,
        // and a mixed pair needs the alias view, so it declines.
        let rec = recursive_alias("mod.A");
        assert_eq!(is_recursive_pair(&rec, &rec), Some(true));
        let int = instance("builtins.int");
        assert_eq!(is_recursive_pair(&int, &int), Some(false));
        assert_eq!(is_recursive_pair(&rec, &int), None);
    }

    #[test]
    fn bind_self_strips_the_first_parameter() {
        // typeops.py:540-641: the non-generic path drops parameter 0 and
        // marks the callable bound.
        let method = callable(vec![ARG_POS, ARG_POS], Vec::new());
        let out = bind_self(&method);
        let Type::CallableType { arg_types, is_bound, .. } = out.unwrap() else {
            panic!("expected a bound callable");
        };
        assert_eq!(arg_types.len(), 1);
        assert!(is_bound);
    }

    #[test]
    fn bind_self_declines_on_a_generic_callable() {
        let variables = vec![type_var(1, "T")];
        let generic = callable(vec![ARG_POS], variables);
        assert_eq!(bind_self(&generic), None);
        let int = instance("builtins.int");
        assert_eq!(bind_self(&int), None);
    }

    #[test]
    fn callable_accessors_read_the_parameter_shape() {
        // types.py:2330-2472: the count and flag accessors.
        let plain = callable(vec![ARG_POS, ARG_OPT], Vec::new());
        assert_eq!(callable_min_args(&plain), Some(1));
        assert_eq!(callable_max_possible_positional_args(&plain), Some(2));
        assert_eq!(callable_is_var_arg(&plain), Some(false));
        assert_eq!(callable_is_kw_arg(&plain), Some(false));
        assert_eq!(callable_is_generic(&plain), Some(false));
        let starred = callable(vec![ARG_POS, ARG_STAR, ARG_STAR2], Vec::new());
        assert_eq!(callable_is_var_arg(&starred), Some(true));
        assert_eq!(callable_is_kw_arg(&starred), Some(true));
        let max = callable_max_possible_positional_args(&starred);
        assert_eq!(max, Some(i64::MAX));
        let generic = callable(vec![ARG_POS], vec![type_var(1, "T")]);
        assert_eq!(callable_is_generic(&generic), Some(true));
    }

    #[test]
    fn callable_accessors_decline_on_a_non_callable() {
        let int = instance("builtins.int");
        assert_eq!(callable_min_args(&int), None);
        assert_eq!(callable_is_var_arg(&int), None);
        assert_eq!(callable_is_kw_arg(&int), None);
        assert_eq!(callable_is_generic(&int), None);
        assert_eq!(callable_max_possible_positional_args(&int), None);
    }

    #[test]
    fn tuple_length_counts_items_and_declines_otherwise() {
        let items = vec![instance("builtins.int"), instance("builtins.str")];
        let tup = tuple(items);
        assert_eq!(tuple_length(&tup), Some(2));
        let int = instance("builtins.int");
        assert_eq!(tuple_length(&int), None);
    }
}
