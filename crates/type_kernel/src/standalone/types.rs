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
//! # `None` is a decline, never an answer
//!
//! The kernel ports mypy's type algebra strangler-fig style: `Some(v)` is
//! the answer mypy gives, `None` means the snapshot facts on hand cannot
//! reproduce it. A standalone caller has no Python to fall back to, so it
//! must treat `None` as an unsupported construct and reject loudly
//! (wave-1 rule 6). Nothing here converts a decline into a guess, and no
//! wrapper below widens the set of cases the kernel decides.
//!
//! # Alias operands decline until a standalone alias store exists
//!
//! `TypeResolver::install_aliases` is crate-private on `typeinfo.rs`, so a
//! caller outside this crate cannot yet install alias snapshots. Any
//! operation below that meets a `Type::TypeAliasType` therefore declines
//! exactly as the hybrid does when no alias view is installed. Wave 2
//! lifts that once `standalone::records` owns a Python-free alias
//! producer.

pub use crate::subtypes::{is_subtype, SubtypeContext};
pub use crate::typeinfo::TypeResolver;
pub use crate::wire::Type;

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
}
