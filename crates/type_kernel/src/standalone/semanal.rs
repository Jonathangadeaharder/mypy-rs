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
//! The exposed families, each named after the mypy function it ports:
//!
//! * the `Any`-rewriting algebra and the type queries of
//!   `mypy/semanal.py` and `mypy/typeanal.py`, which the driver needs to
//!   gate `--disallow-any-explicit` / `--disallow-any-unimported` on a
//!   type alias target and to bind a method's implicit first argument;
//! * the two name-elision predicates of `mypy/sharedparse.py`, which
//!   decide the parameter names a signature displays;
//! * the two record-decidable arms of
//!   `SemanticAnalyzer.lookup_qualified`, the TypeInfo MRO step and the
//!   MypyFile symbol-table chain;
//! * base-class and metaclass resolution: the per-base decisions of
//!   `clean_up_bases_and_infer_type_variables` and
//!   `configure_base_classes`, the `verify_base_classes` /
//!   `verify_duplicate_base_classes` MRO tail, the two
//!   `six`/`future`/`past` compat-helper classifiers, and the
//!   `get_declared_metaclass` / `recalculate_metaclass` decision heads;
//! * type-expression analysis: the branch heads of
//!   `TypeAnalyser.analyze_type_with_type_info`, `analyze_callable_type`,
//!   `anal_type_guard_arg` / `anal_type_is_arg` and the implicit-tuple
//!   message arbitration of `visit_tuple_type`.
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

use crate::semanal_algebra as algebra;
use crate::semanal_bases as bases;
use crate::semanal_lookup as lookup;
use crate::semanal_metaclass as meta;
use crate::semanal_shared as shared;
use crate::typeanal_callable as callable;
use crate::typeanal_info as info;
use crate::typeanal_queries as queries;
use crate::typeanal_special as special;

pub use crate::skeleton_api::{ModuleSnapshot, Type, TypeInfoSnapshot, TypeResolver};

/// `mypy.semanal.make_any_non_explicit` (semanal.py:10161).
///
/// Rewrites every `AnyType(type_of_any=explicit)` inside `t` to
/// `AnyType(special_form)`, because an inlined type-alias target is no
/// longer an explicit `Any`. Lifts
/// `semanal_algebra::make_any_non_explicit_inner`, the same code the
/// `rust_make_any_non_explicit` seam wraps.
pub fn make_any_non_explicit(t: Type) -> Type {
    algebra::make_any_non_explicit_inner(t)
}

/// `mypy.semanal.make_any_non_unimported` (semanal.py:10188).
///
/// Rewrites every `AnyType(type_of_any=from_unimported_type)` inside `t`
/// to `AnyType(special_form)`, and clears `missing_import_name` on every
/// `AnyType` it walks whether or not it rewrote it. Lifts
/// `semanal_algebra::make_any_non_unimported_inner`.
pub fn make_any_non_unimported(t: Type) -> Type {
    algebra::make_any_non_unimported_inner(t)
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
    algebra::replace_implicit_first_type_inner(sig, new)
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
    queries::has_explicit_any_inner(t, queries::EXPLICIT)
}

/// `mypy.typeanal.has_any_from_unimported_type` (typeanal.py:4307).
///
/// The same `ANY_STRATEGY` walk as [`has_explicit_any`], matching
/// `type_of_any == from_unimported_type` instead. `None` defers on a
/// `TypeAliasType`. Lifts `typeanal_queries::has_explicit_any_inner`.
pub fn has_any_from_unimported_type(t: &Type) -> Option<bool> {
    queries::has_explicit_any_inner(t, queries::FROM_UNIMPORTED_TYPE)
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
    shared::special_function_elide_names_inner(name)
}

/// `mypy.sharedparse.argument_elide_name` (sharedparse.py:113).
///
/// Whether a parameter named `name` is elided from a signature: a leading
/// dunder that is not also a trailing one. `None` (an unnamed parameter)
/// is never elided. Lifts `semanal_shared::argument_elide_name_inner`.
pub fn argument_elide_name(name: Option<&str>) -> bool {
    shared::argument_elide_name_inner(name)
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
    match lookup::find_member_in_mro(resolver, snap, name) {
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
    match lookup::walk_mypyfile_chain(resolver, &parts, module_fullname) {
        lookup::WalkOutcome::Resolved(full) => LookupOutcome::Resolved(full),
        lookup::WalkOutcome::NotFound => LookupOutcome::NotFound,
        lookup::WalkOutcome::Defer => LookupOutcome::Deferred,
    }
}

/// What `SemanticAnalyzer.clean_up_bases_and_infer_type_variables`
/// (semanal.py:2709) does with one base expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseCleanup {
    /// Not a `Generic[...]` / `Protocol[...]` declaration: the base stays
    /// in `defn.base_type_exprs` and is analyzed normally.
    Keep,
    /// `typing.Generic`, with or without arguments. Removed; its type
    /// variables are declared for the class and `is_protocol` is unchanged
    /// (semanal.py:2824: a bare `Generic` is still a declaration).
    Generic,
    /// `typing.Protocol` / `typing_extensions.Protocol` with arguments.
    /// Removed; its type variables are declared and `is_protocol` is set.
    ProtocolGeneric,
    /// A bare `Protocol` name. Removed; `is_protocol` is set and no type
    /// variables are declared.
    BareProtocol,
}

/// The per-base decision of
/// `SemanticAnalyzer.clean_up_bases_and_infer_type_variables`
/// (semanal.py:2817-2843 + 2768-2774).
///
/// `fullname` is the resolved `sym.node.fullname` of an `UnboundType` base,
/// or `None` when the lookup failed or the node is missing.
/// `in_protocol_names` is whether that fullname is in `mypy.types`'
/// `PROTOCOL_NAMES`; the set travels from the caller so it stays
/// single-sourced, exactly as the hybrid shim passes it. `has_args` is
/// `bool(base.args)`.
///
/// Branch order mirrors Python exactly: `typing.Generic` wins first, then
/// the `Protocol` names split on whether arguments are present. Always
/// decidable. Lifts `semanal_bases::clean_up_bases_inner`.
pub fn clean_up_bases(
    fullname: Option<&str>,
    in_protocol_names: bool,
    has_args: bool,
) -> BaseCleanup {
    match bases::clean_up_bases_inner(fullname, in_protocol_names, has_args) {
        bases::ACTION_GENERIC => BaseCleanup::Generic,
        bases::ACTION_PROTOCOL_GENERIC => BaseCleanup::ProtocolGeneric,
        bases::ACTION_BARE_PROTOCOL => BaseCleanup::BareProtocol,
        bases::ACTION_KEEP => BaseCleanup::Keep,
        other => unreachable!("clean_up_bases_inner returned the tag {other}"),
    }
}

/// The option gates `SemanticAnalyzer.configure_base_classes`
/// (semanal.py:3348-3381) reads before validating one base. The derived
/// `Default` is every gate off, which is mypy's default for all three
/// options plus a non-stub file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BaseClassOptions {
    /// `--disallow-subclassing-any`: an `Any` base is an error, not just a
    /// `fallback_to_any` marker.
    pub disallow_subclassing_any: bool,
    /// `--disallow-any-unimported`: run the `has_any_from_unimported_type`
    /// walk over the base.
    pub disallow_any_unimported: bool,
    /// `--disallow-any-explicit`: run the `has_explicit_any` walk over the
    /// base.
    pub disallow_any_explicit: bool,
    /// Whether the file is a typeshed stub, which suppresses the
    /// explicit-`Any` report.
    pub is_typeshed_stub_file: bool,
}

/// The per-base kind `configure_base_classes` decides from the
/// `ProperType` isinstance chain (semanal.py:3349-3372).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseKind {
    /// A `TupleType` base: the caller runs `configure_tuple_base_class`.
    Tuple,
    /// A plain `Instance` base: append it to `info.bases`.
    Instance,
    /// An `Instance` base that is a `NewType`: mypy fails
    /// 'Cannot subclass "NewType"' and still appends the base.
    NewTypeFail,
    /// An `AnyType` base with `disallow_subclassing_any` off: only
    /// `info.fallback_to_any` is set.
    AnyAllowed,
    /// An `AnyType` base with `disallow_subclassing_any` on: mypy also
    /// fails "Class cannot subclass ...".
    AnyRejected,
    /// A `TypedDictType` base: append `base.fallback`.
    TypedDictFallback,
    /// Anything else: mypy fails "Invalid base class ..." and sets
    /// `fallback_to_any`.
    Invalid,
}

/// One base's `configure_base_classes` decision plus the two `Any` reports
/// the caller must emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseConfiguration {
    /// What kind of base this is, and so which effect mypy applies.
    pub kind: BaseKind,
    /// Emit `msg.unimported_type_becomes_any` for this base. Only ever true
    /// when `BaseClassOptions::disallow_any_unimported` is set.
    pub report_unimported_any: bool,
    /// Emit `msg.explicit_any` for this base. Only ever true when
    /// `disallow_any_explicit` is set and the file is not a typeshed stub.
    pub report_explicit_any: bool,
}

/// `SemanticAnalyzer.configure_base_classes` for one base
/// (semanal.py:3348-3381).
///
/// `base` must be the base's `ProperType` (mypy runs `get_proper_type`
/// before the isinstance chain), so its top-level variant decides the kind
/// one to one. The two `Any` walks run under exactly the gates Python uses:
/// `has_any_from_unimported_type` only when `disallow_any_unimported`,
/// `has_explicit_any` only when `disallow_any_explicit` and the file is not
/// a typeshed stub.
///
/// `None` means a walk reached a `TypeAliasType` the records cannot expand
/// (see the module docs); the caller must reject loudly. Every side effect
/// stays with the caller: `configure_tuple_base_class`, the two `fail`
/// emissions, `info.fallback_to_any` and the `info.bases` writes.
///
/// Lifts the per-base body of `semanal_bases::rust_classify_configure_bases`
/// minus its wire decode: `wire_base_kind`, `has_any_from_unimported_inner`,
/// `typeanal_queries::has_explicit_any_inner` and
/// `classify_configure_base_inner`.
pub fn configure_base_class(
    base: &Type,
    is_newtype: bool,
    opts: &BaseClassOptions,
) -> Option<BaseConfiguration> {
    let kind = bases::wire_base_kind(base);
    let unimported_any = if opts.disallow_any_unimported {
        bases::has_any_from_unimported_inner(base)
    } else {
        Some(false)
    };
    let explicit_any = if opts.disallow_any_explicit && !opts.is_typeshed_stub_file {
        queries::has_explicit_any_inner(base, queries::EXPLICIT)
    } else {
        Some(false)
    };
    let decided = bases::classify_configure_base_inner(
        kind,
        is_newtype,
        opts.disallow_subclassing_any,
        unimported_any,
        explicit_any,
    )?;
    let configuration = BaseConfiguration {
        kind: base_kind(decided.0),
        report_unimported_any: decided.1,
        report_explicit_any: decided.2,
    };
    Some(configuration)
}

/// `CONFIGURE_*` tag to [`BaseKind`]. The tag set is closed, so an unknown
/// value is a kernel contract break rather than an input error.
fn base_kind(tag: i64) -> BaseKind {
    match tag {
        bases::CONFIGURE_TUPLE => BaseKind::Tuple,
        bases::CONFIGURE_INSTANCE => BaseKind::Instance,
        bases::CONFIGURE_INSTANCE_NEWTYPE_FAIL => BaseKind::NewTypeFail,
        bases::CONFIGURE_ANY_OK => BaseKind::AnyAllowed,
        bases::CONFIGURE_ANY_FAIL => BaseKind::AnyRejected,
        bases::CONFIGURE_TYPEDDICT_FALLBACK => BaseKind::TypedDictFallback,
        bases::CONFIGURE_INVALID_BASE => BaseKind::Invalid,
        other => unreachable!("classify_configure_base_inner returned {other}"),
    }
}

/// The `verify_base_classes` + `verify_duplicate_base_classes` tail of
/// `configure_base_classes` (semanal.py:3390-3397 + 3512-3526).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MroTail {
    /// At least one base cycles back to the class. mypy fails "Cycle in
    /// inheritance hierarchy" once per index in `cyclic`, in `info.bases`
    /// order, and calls `set_dummy_mro`.
    DummyHierarchy { cyclic: Vec<usize> },
    /// A duplicate direct base. mypy fails 'Duplicate base class "..."' and
    /// calls `set_any_mro`, then `calculate_class_mro`.
    AnyHierarchy { duplicate: String },
    /// A clean hierarchy: the caller just runs `calculate_class_mro`.
    Proceed,
}

/// The MRO tail decision of `SemanticAnalyzer.configure_base_classes`
/// (semanal.py:3390-3397, folding `verify_base_classes` and
/// `verify_duplicate_base_classes` at :3512-3526).
///
/// `cyclic_bases` holds the indices into `info.bases` whose base cycles back
/// to the class being configured (mypy's `is_base_class` walk);
/// `duplicate_base` is the name `find_duplicate(direct_base_classes())`
/// returned. Cyclic bases win, matching Python's early return, then a
/// duplicate, else proceed. Always decidable. Lifts
/// `semanal_bases::configure_mro_tail_inner`.
pub fn configure_mro_tail(cyclic_bases: &[usize], duplicate_base: Option<&str>) -> MroTail {
    let (tag, cyclic, duplicate) = bases::configure_mro_tail_inner(cyclic_bases, duplicate_base);
    match tag {
        bases::MRO_DUMMY => MroTail::DummyHierarchy { cyclic },
        bases::MRO_ANY => match duplicate {
            Some(duplicate) => MroTail::AnyHierarchy { duplicate },
            None => unreachable!("the MRO_ANY tag carries a duplicate name"),
        },
        bases::MRO_PROCEED => MroTail::Proceed,
        other => unreachable!("configure_mro_tail_inner returned the tag {other}"),
    }
}

/// Whether a base expression is a `six.with_metaclass(M, B1, ...)`
/// compat-helper call: the base side of
/// `SemanticAnalyzer.infer_metaclass_and_bases_from_compat_helpers`
/// (semanal.py:3321-3338).
///
/// `fullname` is the callee's `fullname`, which the caller must have
/// populated by analyzing the base expression first. Matches
/// `six.with_metaclass`, `future.utils.with_metaclass` and
/// `past.utils.with_metaclass` with at least one argument, all positional.
/// On `true` mypy sets `with_meta_expr = args[0]` and rewrites
/// `defn.base_type_exprs = args[1:]`. Always decidable. Lifts
/// `semanal_bases::classify_with_metaclass_inner`.
pub fn compat_helper_with_metaclass(
    fullname: Option<&str>,
    args_len: usize,
    all_positional: bool,
) -> bool {
    let tag = bases::classify_with_metaclass_inner(fullname, args_len, all_positional);
    tag == bases::ACTION_WITH_METACLASS
}

/// Whether a decorator is `@six.add_metaclass(M)`: the decorator side of
/// `SemanticAnalyzer.infer_metaclass_and_bases_from_compat_helpers`
/// (semanal.py:3369-3379).
///
/// Matches `six.add_metaclass` with exactly one positional argument. On
/// `true` mypy sets `add_meta_expr = args[0]` and stops scanning the
/// decorators. Always decidable. Lifts
/// `semanal_bases::classify_add_metaclass_inner`.
pub fn compat_helper_add_metaclass(
    fullname: Option<&str>,
    args_len: usize,
    arg0_positional: bool,
) -> bool {
    let tag = bases::classify_add_metaclass_inner(fullname, args_len, arg0_positional);
    tag == bases::ACTION_ADD_METACLASS
}

/// The resolved facts `SemanticAnalyzer.get_declared_metaclass`
/// (semanal.py:3767-3835) gates on.
///
/// The hybrid shim reads each field from a live Python object through
/// PyO3; a standalone caller supplies them from its own records, which is
/// the substitution wave-1 rule 4 asks for. `None` in an `Option` field
/// means "the fact could not be read", and the gate that needs it defers.
/// The derived `Default` is therefore the all-unreadable state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredMetaclassFacts {
    /// The metaclass expression's dotted name, or `None` when it is not a
    /// `NameExpr` / `MemberExpr` chain.
    pub mc_name: Option<String>,
    /// `lookup_qualified` returned `None`.
    pub sym_missing: bool,
    /// The symbol node is a `Var`.
    pub sym_is_var: Option<bool>,
    /// The symbol node is a `PlaceholderNode`.
    pub sym_is_placeholder: Option<bool>,
    /// The `Var` symbol's proper type is an `AnyType`. Only consulted when
    /// `sym_is_var` is true.
    pub var_any: Option<bool>,
    /// The resolved symbol is a `TypeInfo`.
    pub meta_is_typeinfo: Option<bool>,
    /// That `TypeInfo` has a `tuple_type`. Only consulted when
    /// `meta_is_typeinfo` is true.
    pub meta_has_tuple_type: Option<bool>,
    /// That `TypeInfo` inherits from `type`.
    pub meta_is_metaclass: Option<bool>,
}

/// The gate `get_declared_metaclass` stopped at, each with the mypy effect
/// the caller must apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredMetaclass {
    /// A valid metaclass `TypeInfo`: mypy runs `fill_typevars` and returns
    /// `(inst, False, False)`.
    Valid,
    /// The metaclass name is not representable: mypy fails 'Dynamic
    /// metaclass not supported for "..."' and returns `(None, False, True)`.
    Dynamic,
    /// `lookup_qualified` failed; the name error is reported elsewhere.
    NameError,
    /// A `Var` symbol whose proper type is `Any`: mypy fails 'Class cannot
    /// use "..." as a metaclass' only under `disallow_subclassing_any`.
    AnyVar,
    /// A `PlaceholderNode` symbol: mypy returns `(None, True, False)` and
    /// the class deferral runs.
    Placeholder,
    /// The symbol is not a `TypeInfo`, or is a tuple-named class: mypy
    /// fails 'Invalid metaclass "..."'.
    Invalid,
    /// The class does not inherit from `type`: mypy fails 'Metaclasses not
    /// inheriting from "type" are not supported'.
    NotMetaclass,
}

/// `SemanticAnalyzer.get_declared_metaclass` decision head
/// (semanal.py:3767-3835).
///
/// Runs mypy's strictly sequential gate chain over [`DeclaredMetaclassFacts`]
/// and returns the gate it stopped at. `None` means a fact was unreadable at
/// the gate that needed it, which is the module-docs deferral. Every side
/// effect stays with the caller: the four `self.fail` calls, the
/// `disallow_subclassing_any` option gate on [`DeclaredMetaclass::AnyVar`],
/// and the `fill_typevars` construction.
///
/// Lifts `semanal_metaclass::classify_declared_metaclass_inner`.
pub fn get_declared_metaclass(facts: &DeclaredMetaclassFacts) -> Option<DeclaredMetaclass> {
    let tag = meta::classify_declared_metaclass_inner(
        facts.mc_name.as_deref(),
        facts.sym_missing,
        facts.sym_is_var,
        facts.sym_is_placeholder,
        facts.var_any,
        facts.meta_is_typeinfo,
        facts.meta_has_tuple_type,
        facts.meta_is_metaclass,
    )?;
    Some(declared_metaclass(tag))
}

/// `META_*` tag to [`DeclaredMetaclass`]. The tag set is closed.
fn declared_metaclass(tag: i64) -> DeclaredMetaclass {
    match tag {
        meta::META_OK => DeclaredMetaclass::Valid,
        meta::META_DYNAMIC => DeclaredMetaclass::Dynamic,
        meta::META_NAME_ERROR => DeclaredMetaclass::NameError,
        meta::META_ANY => DeclaredMetaclass::AnyVar,
        meta::META_DEFER => DeclaredMetaclass::Placeholder,
        meta::META_INVALID => DeclaredMetaclass::Invalid,
        meta::META_NOT_METACLASS => DeclaredMetaclass::NotMetaclass,
        other => unreachable!("classify_declared_metaclass_inner returned {other}"),
    }
}

/// The facts `SemanticAnalyzer.recalculate_metaclass` (semanal.py:3837)
/// scans. mypy's two unconditional writes (`declared_metaclass`,
/// `metaclass_type = calculate_metaclass_type()`) happen before this is
/// consulted, so `meta_present` describes the recalculated metaclass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecalculateMetaclassFacts {
    /// Any class in the MRO is a protocol.
    pub any_protocol_mro: bool,
    /// The class has a metaclass at all (not `None`).
    pub meta_present: bool,
    /// That metaclass is `builtins.type`. Only consulted when
    /// `any_protocol_mro` and `meta_present` both hold.
    pub meta_is_builtins_type: Option<bool>,
    /// That metaclass has the `enum.EnumMeta` base.
    pub meta_is_enum: Option<bool>,
    /// The class definition is generic (`defn.type_vars` non-empty).
    pub type_vars_nonempty: bool,
}

/// The `recalculate_metaclass` tail decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecalculatedMetaclass {
    /// Nothing to do.
    Unchanged,
    /// A protocol in the MRO with no metaclass or the default one: mypy
    /// installs `named_type_or_none("abc.ABCMeta", [])`.
    AbcMeta,
    /// The metaclass derives from `enum.EnumMeta`: mypy sets
    /// `info.is_enum = True`.
    IsEnum,
    /// The same, on a generic class definition: mypy also fails "Enum class
    /// cannot be generic".
    EnumGenericFail,
}

/// `SemanticAnalyzer.recalculate_metaclass` decision head
/// (semanal.py:3837-3856).
///
/// Folds mypy's protocol-MRO scan and enum scan into one exclusive answer.
/// The arms are exclusive by construction: when the `abc.ABCMeta`
/// replacement fires it installs `abc.ABCMeta`, never an enum metaclass, or
/// leaves a `None` / `builtins.type` metaclass in place, so the enum scan
/// cannot fire on the same class. Branch order mirrors Python: the
/// protocol-MRO block runs before the enum scan, which reads the
/// post-replacement metaclass.
///
/// `None` is the module-docs deferral. Every side effect stays with the
/// caller: the `named_type_or_none("abc.ABCMeta")` write, `is_enum = True`
/// and the "Enum class cannot be generic" fail. Lifts
/// `semanal_metaclass::classify_recalculate_metaclass_inner`.
pub fn recalculate_metaclass(facts: &RecalculateMetaclassFacts) -> Option<RecalculatedMetaclass> {
    let tag = meta::classify_recalculate_metaclass_inner(
        facts.any_protocol_mro,
        facts.meta_present,
        facts.meta_is_builtins_type,
        facts.meta_is_enum,
        facts.type_vars_nonempty,
    )?;
    Some(recalculated_metaclass(tag))
}

/// `RECALC_*` tag to [`RecalculatedMetaclass`]. The tag set is closed.
fn recalculated_metaclass(tag: i64) -> RecalculatedMetaclass {
    match tag {
        meta::RECALC_OK => RecalculatedMetaclass::Unchanged,
        meta::RECALC_ABCMETA => RecalculatedMetaclass::AbcMeta,
        meta::RECALC_IS_ENUM => RecalculatedMetaclass::IsEnum,
        meta::RECALC_ENUM_GENERIC_FAIL => RecalculatedMetaclass::EnumGenericFail,
        other => unreachable!("classify_recalculate_metaclass_inner returned {other}"),
    }
}

/// The facts `TypeAnalyser.analyze_type_with_type_info` binds an unbound
/// type against. The hybrid shim reads each from the live `TypeInfo`
/// (nodes.py:3964-3967); a standalone caller supplies them from records.
/// The derived `Default` is the plain-reference case: no arguments and none
/// of the three special fields set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeInfoFacts {
    /// `len(t.args)`.
    pub args_len: i64,
    /// `info.tuple_type is not None`.
    pub tuple_type_not_none: bool,
    /// `info.special_alias is not None`.
    pub special_alias_not_none: bool,
    /// `info.typeddict_type is not None`.
    pub typeddict_type_not_none: bool,
}

/// The terminal branch of `analyze_type_with_type_info`, each named for the
/// typeanal.py branch the caller must then execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeInfoBranch {
    /// typeanal.py:1176-1178: `tuple[...]` with arguments builds a
    /// `TupleType`.
    Tuple,
    /// typeanal.py:1204-1207: `librt.vecs.vec` with an invalid item type
    /// becomes `Any(from_error)`.
    Vec,
    /// typeanal.py:1228-1253: a named-tuple base with no special alias.
    TupleTail,
    /// typeanal.py:1232-1250: a tuple base that is also a special alias.
    TupleTailAlias,
    /// typeanal.py:1254-1279: a TypedDict base with no special alias.
    TypedDictTail,
    /// typeanal.py:1258-1275: a TypedDict base that is also an alias.
    TypedDictTailAlias,
    /// typeanal.py:1281-1287: `types.NoneType` fails and builds `NoneType`.
    NoneType,
    /// typeanal.py:1289: a plain `Instance`.
    Instance,
}

/// `mypy.typeanal.TypeAnalyser.analyze_type_with_type_info`
/// (typeanal.py:1166-1289).
///
/// Binds an unbound type that resolved to a `TypeInfo`: `fullname` is
/// `info.fullname` and `facts` carries the argument count plus which of
/// `tuple_type` / `special_alias` / `typeddict_type` are set. Mirrors the
/// branch order of typeanal.py:1176-1289 exactly.
///
/// Every fact is a scalar, so this always decides: there is no deferral and
/// no `Option`. The caller keeps every side effect mypy performs on the
/// branch it lands on, namely the `vec` item-type check, the argument-count
/// validation, the tuple and TypedDict tails, and the `types.NoneType`
/// error.
///
/// Lifts `typeanal_info::classify_type_with_info_inner`.
pub fn analyze_type_with_type_info(fullname: &str, facts: &TypeInfoFacts) -> TypeInfoBranch {
    let decided = info::classify_type_with_info_inner(
        fullname,
        facts.args_len,
        facts.tuple_type_not_none,
        facts.special_alias_not_none,
        facts.typeddict_type_not_none,
    );
    match decided {
        Some(tag) => type_info_branch(tag),
        None => unreachable!("every fact is a scalar, so this decides"),
    }
}

/// `typeanal_info::TAG_*` to [`TypeInfoBranch`]. The tag set is closed.
fn type_info_branch(tag: i64) -> TypeInfoBranch {
    match tag {
        info::TAG_TUPLE => TypeInfoBranch::Tuple,
        info::TAG_VEC => TypeInfoBranch::Vec,
        info::TAG_TUPLE_TAIL => TypeInfoBranch::TupleTail,
        info::TAG_TUPLE_TAIL_ALIAS => TypeInfoBranch::TupleTailAlias,
        info::TAG_TYPEDDICT_TAIL => TypeInfoBranch::TypedDictTail,
        info::TAG_TYPEDDICT_TAIL_ALIAS => TypeInfoBranch::TypedDictTailAlias,
        info::TAG_NONE_TYPE => TypeInfoBranch::NoneType,
        info::TAG_INSTANCE => TypeInfoBranch::Instance,
        other => unreachable!("unknown analyze_type_with_type_info tag {other}"),
    }
}

/// The facts `TypeAnalyser.analyze_callable_type` dispatches on. The
/// derived `Default` is the bare `Callable` case: zero arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallableFacts {
    /// `len(t.args)`.
    pub arg_count: i64,
    /// `isinstance(t.args[0], TypeList)`; only read when `arg_count == 2`.
    pub arg0_is_type_list: bool,
    /// `isinstance(t.args[0], EllipsisType)`; only read at `arg_count == 2`.
    pub arg0_is_ellipsis: bool,
    /// `options.disallow_any_generics`, which selects the invalid-arity
    /// message.
    pub disallow_any_generics: bool,
}

/// The terminal branch of `analyze_callable_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableBranch {
    /// typeanal.py:2333-2335: a bare `Callable` is `Callable[..., Any]`.
    BareCallable,
    /// typeanal.py:2337-2344: `Callable[[ARG, ...], RET]`.
    TypeList,
    /// typeanal.py:2345-2350: `Callable[..., RET]`.
    Ellipsis,
    /// typeanal.py:2351-2376: `Callable[P, RET]`, the ParamSpec form.
    ParamSpec,
    /// typeanal.py:2377-2379: an invalid arity under
    /// `disallow_any_generics`.
    InvalidArityDisallowed,
    /// typeanal.py:2380-2382: an invalid arity with any generics allowed.
    InvalidArityAllowed,
}

/// `mypy.typeanal.TypeAnalyser.analyze_callable_type` (typeanal.py:2330).
///
/// The two-level dispatch head: `len(t.args)`, where 0 is a bare
/// `Callable`, 2 is the normal form and anything else is an invalid arity;
/// then, inside the `2` arm, the kind of `t.args[0]` (`TypeList`,
/// `EllipsisType`, or the ParamSpec form). Every fact is a scalar, so this
/// always decides.
///
/// The caller keeps the side effects mypy applies on the branch: the
/// `tvar_scope` entry, the `analyze_callable_args*` variants, and the
/// `fail` / `note` emissions.
///
/// Lifts `typeanal_callable::classify_analyze_callable_type_inner`.
pub fn analyze_callable_type(facts: &CallableFacts) -> CallableBranch {
    let decided = callable::classify_analyze_callable_type_inner(
        facts.arg_count,
        facts.arg0_is_type_list,
        facts.arg0_is_ellipsis,
        facts.disallow_any_generics,
    );
    match decided {
        Some(tag) => callable_branch(tag),
        None => unreachable!("every fact is a scalar, so this decides"),
    }
}

/// `typeanal_callable::TAG_*` to [`CallableBranch`]. The tag set is closed.
fn callable_branch(tag: i64) -> CallableBranch {
    match tag {
        callable::TAG_BARE_CALLABLE => CallableBranch::BareCallable,
        callable::TAG_TYPE_LIST => CallableBranch::TypeList,
        callable::TAG_ELLIPSIS => CallableBranch::Ellipsis,
        callable::TAG_PARAMSPEC => CallableBranch::ParamSpec,
        callable::TAG_INVALID_DISALLOW => CallableBranch::InvalidArityDisallowed,
        callable::TAG_INVALID_ALLOW => CallableBranch::InvalidArityAllowed,
        other => unreachable!("unknown analyze_callable_type tag {other}"),
    }
}

/// The facts `anal_type_guard_arg` / `anal_type_is_arg` gate on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeGuardFacts {
    /// `len(t.args)`.
    pub args_len: usize,
    /// Whether to match the `TypeIs` name set instead of `TypeGuard`'s.
    pub is_typeis: bool,
}

/// The outcome of `anal_type_guard_arg` / `anal_type_is_arg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeGuardBranch {
    /// The fullname is not in the family, so mypy's wrapper returns `None`
    /// and the caller continues the special-form chain.
    NotAGuard,
    /// typeanal.py:2009-2033: the arity is not 1, so mypy fails
    /// `INVALID_TYPE` and builds `Any(from_error)`.
    ArityFail,
    /// The arity is 1: mypy analyzes `t.args[0]`.
    Recurse,
}

/// `mypy.typeanal.TypeAnalyser.anal_type_guard_arg` and `anal_type_is_arg`
/// (typeanal.py:2009-2033).
///
/// Two-step decision: family membership, selected by `facts.is_typeis`
/// between the `TypeIs` names and the `TypeGuard` names (each in `typing`
/// and `typing_extensions`), then the arity gate. `fullname` is what the
/// caller resolved through `lookup_qualified`. Every fact is a scalar, so
/// this always decides.
///
/// Lifts `typeanal_special::classify_type_guard_arg_inner`.
pub fn analyze_type_guard_arg(fullname: &str, facts: &TypeGuardFacts) -> TypeGuardBranch {
    let args_len = facts.args_len;
    let is_typeis = facts.is_typeis;
    let decided = special::classify_type_guard_arg_inner(fullname, args_len, is_typeis);
    match decided {
        Some(tag) => type_guard_branch(tag),
        None => unreachable!("every fact is a scalar, so this decides"),
    }
}

/// `typeanal_special::TAG_GUARD_*` to [`TypeGuardBranch`]. Closed tag set.
fn type_guard_branch(tag: i64) -> TypeGuardBranch {
    match tag {
        special::TAG_GUARD_NOT_GUARD => TypeGuardBranch::NotAGuard,
        special::TAG_GUARD_FAIL => TypeGuardBranch::ArityFail,
        special::TAG_GUARD_RECURSE => TypeGuardBranch::Recurse,
        other => unreachable!("unknown anal_type_guard_arg tag {other}"),
    }
}

/// The facts `visit_tuple_type` arbitrates its implicit-tuple message on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImplicitTupleFacts {
    /// `t.implicit`.
    pub implicit: bool,
    /// The analyzer flag that permits a tuple literal.
    pub allow_tuple_literal: bool,
    /// `len(t.items)`.
    pub items_len: usize,
}

/// The note `TypeAnalyser.visit_tuple_type` picks for an implicit tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplicitTupleNote {
    /// The error head does not fire: the normal reconstruction path
    /// (`named_type` plus `anal_array`) runs.
    Reconstruct,
    /// `len(t.items) == 0`: the `Tuple[()]` suggestion.
    SuggestEmptyTuple,
    /// `len(t.items) == 1`: the spurious-comma suggestion.
    SuggestSpuriousComma,
    /// `len(t.items) > 1`: the `Tuple[T1, ..., Tn]` suggestion.
    SuggestTupleOfItems,
}

/// `mypy.typeanal.TypeAnalyser.visit_tuple_type` implicit-tuple message
/// arbitration (typeanal.py:2041-2058).
///
/// The error head fires only when `t.implicit` is set and
/// `allow_tuple_literal` is off; inside the head the note is chosen by
/// `len(t.items)`. Every fact is a scalar, so this always decides. The
/// caller keeps the `fail` and `note` emissions and the normal
/// reconstruction path.
///
/// Lifts `typeanal_special::classify_tuple_type_implicit_inner`.
pub fn visit_tuple_type_implicit(facts: &ImplicitTupleFacts) -> ImplicitTupleNote {
    let decided = special::classify_tuple_type_implicit_inner(
        facts.implicit,
        facts.allow_tuple_literal,
        facts.items_len,
    );
    match decided {
        Some(tag) => implicit_tuple_note(tag),
        None => unreachable!("every fact is a scalar, so this decides"),
    }
}

/// `typeanal_special::TAG_TUPLE_*` to [`ImplicitTupleNote`]. Closed set.
fn implicit_tuple_note(tag: i64) -> ImplicitTupleNote {
    match tag {
        special::TAG_TUPLE_OK => ImplicitTupleNote::Reconstruct,
        special::TAG_TUPLE_EMPTY => ImplicitTupleNote::SuggestEmptyTuple,
        special::TAG_TUPLE_SINGLE => ImplicitTupleNote::SuggestSpuriousComma,
        special::TAG_TUPLE_MULTI => ImplicitTupleNote::SuggestTupleOfItems,
        other => unreachable!("unknown visit_tuple_type tag {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typeanal_queries::{EXPLICIT, FROM_UNIMPORTED_TYPE};
    use std::collections::{HashMap, HashSet};

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

    fn tuple_type() -> Type {
        Type::TupleType {
            partial_fallback: Box::new(instance("builtins.tuple", vec![])),
            items: Vec::new(),
            implicit: false,
        }
    }

    fn typed_dict() -> Type {
        Type::TypedDictType {
            fallback: Box::new(instance("builtins.dict", vec![])),
            items: Vec::new(),
            required_keys: HashSet::new(),
            readonly_keys: HashSet::new(),
            is_closed: false,
        }
    }

    /// The facts of a resolved, valid metaclass: a named symbol that is a
    /// non-tuple `TypeInfo` inheriting from `type`. Each test overrides one
    /// field to reach a different gate.
    fn declared_facts() -> DeclaredMetaclassFacts {
        DeclaredMetaclassFacts {
            mc_name: Some("mod.Meta".to_string()),
            sym_missing: false,
            sym_is_var: Some(false),
            sym_is_placeholder: Some(false),
            var_any: Some(false),
            meta_is_typeinfo: Some(true),
            meta_has_tuple_type: Some(false),
            meta_is_metaclass: Some(true),
        }
    }

    /// `configure_base_class` narrowed to the kind, asserting the decision
    /// was reachable at all (a `None` here means an unexpected deferral).
    fn base_kind_of(base: &Type, is_newtype: bool, opts: &BaseClassOptions) -> BaseKind {
        let decided = configure_base_class(base, is_newtype, opts);
        decided.unwrap().kind
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
            Type::AnyType {
                missing_import_name,
                ..
            } => missing_import_name.as_deref(),
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

    #[test]
    fn clean_up_bases_declares_generic_and_protocol_bases() {
        let generic = clean_up_bases(Some("typing.Generic"), false, false);
        assert_eq!(generic, BaseCleanup::Generic);
        let subscripted = clean_up_bases(Some("typing.Protocol"), true, true);
        assert_eq!(subscripted, BaseCleanup::ProtocolGeneric);
        let bare = clean_up_bases(Some("typing.Protocol"), true, false);
        assert_eq!(bare, BaseCleanup::BareProtocol);
    }

    #[test]
    fn clean_up_bases_keeps_an_ordinary_or_unresolved_base() {
        let plain = clean_up_bases(Some("mod.Base"), false, false);
        assert_eq!(plain, BaseCleanup::Keep);
        let unresolved = clean_up_bases(None, true, true);
        assert_eq!(unresolved, BaseCleanup::Keep);
    }

    #[test]
    fn configure_base_class_splits_an_instance_on_the_newtype_flag() {
        let base = instance("builtins.object", vec![]);
        let opts = BaseClassOptions::default();
        assert_eq!(base_kind_of(&base, false, &opts), BaseKind::Instance);
        assert_eq!(base_kind_of(&base, true, &opts), BaseKind::NewTypeFail);
    }

    #[test]
    fn configure_base_class_gates_the_explicit_any_report() {
        let base = any(EXPLICIT);
        let off_opts = BaseClassOptions::default();
        let off = configure_base_class(&base, false, &off_opts);
        assert!(!off.unwrap().report_explicit_any);
        let opts = BaseClassOptions {
            disallow_any_explicit: true,
            ..Default::default()
        };
        let on = configure_base_class(&base, false, &opts);
        let on = on.unwrap();
        assert_eq!(on.kind, BaseKind::AnyAllowed);
        assert!(on.report_explicit_any);
    }

    #[test]
    fn configure_base_class_rejects_an_any_base_under_the_option() {
        let opts = BaseClassOptions {
            disallow_subclassing_any: true,
            ..Default::default()
        };
        let base = any(EXPLICIT);
        assert_eq!(base_kind_of(&base, false, &opts), BaseKind::AnyRejected);
    }

    #[test]
    fn configure_base_class_kinds_follow_the_proper_type_chain() {
        let opts = BaseClassOptions::default();
        let tuple = base_kind_of(&tuple_type(), false, &opts);
        assert_eq!(tuple, BaseKind::Tuple);
        let dict = base_kind_of(&typed_dict(), false, &opts);
        assert_eq!(dict, BaseKind::TypedDictFallback);
        let invalid = base_kind_of(&Type::NoneType, false, &opts);
        assert_eq!(invalid, BaseKind::Invalid);
    }

    #[test]
    fn configure_base_class_defers_on_an_aliased_base() {
        let opts = BaseClassOptions {
            disallow_any_unimported: true,
            ..Default::default()
        };
        let base = alias("mod.A");
        assert_eq!(configure_base_class(&base, false, &opts), None);
    }

    #[test]
    fn configure_mro_tail_lets_a_clean_hierarchy_proceed() {
        assert_eq!(configure_mro_tail(&[], None), MroTail::Proceed);
    }

    #[test]
    fn configure_mro_tail_names_a_duplicate_base() {
        let tail = configure_mro_tail(&[], Some("mod.Base"));
        let expected = MroTail::AnyHierarchy {
            duplicate: "mod.Base".to_string(),
        };
        assert_eq!(tail, expected);
    }

    #[test]
    fn configure_mro_tail_lets_a_cycle_win_over_a_duplicate() {
        let tail = configure_mro_tail(&[1, 2], Some("mod.Base"));
        match tail {
            MroTail::DummyHierarchy { cyclic } => assert_eq!(cyclic, vec![1usize, 2usize]),
            other => panic!("expected a dummy hierarchy, got {other:?}"),
        }
    }

    #[test]
    fn compat_helper_with_metaclass_matches_the_three_helper_names() {
        let six = compat_helper_with_metaclass(Some("six.with_metaclass"), 2, true);
        assert!(six);
        let future = compat_helper_with_metaclass(Some("future.utils.with_metaclass"), 1, true);
        assert!(future);
        let past = compat_helper_with_metaclass(Some("past.utils.with_metaclass"), 1, true);
        assert!(past);
    }

    #[test]
    fn compat_helper_with_metaclass_rejects_a_bad_arity_or_kind() {
        assert!(!compat_helper_with_metaclass(
            Some("six.with_metaclass"),
            0,
            true
        ));
        assert!(!compat_helper_with_metaclass(
            Some("six.with_metaclass"),
            2,
            false
        ));
        assert!(!compat_helper_with_metaclass(Some("mod.NotMeta"), 2, true));
        assert!(!compat_helper_with_metaclass(None, 2, true));
    }

    #[test]
    fn compat_helper_add_metaclass_matches_only_six_add_metaclass() {
        assert!(compat_helper_add_metaclass(
            Some("six.add_metaclass"),
            1,
            true
        ));
    }

    #[test]
    fn compat_helper_add_metaclass_rejects_arity_kind_and_name() {
        assert!(!compat_helper_add_metaclass(
            Some("six.add_metaclass"),
            2,
            true
        ));
        assert!(!compat_helper_add_metaclass(
            Some("six.add_metaclass"),
            1,
            false
        ));
        assert!(!compat_helper_add_metaclass(
            Some("six.with_metaclass"),
            1,
            true
        ));
    }

    #[test]
    fn get_declared_metaclass_accepts_a_valid_metaclass() {
        let facts = declared_facts();
        let decided = get_declared_metaclass(&facts);
        assert_eq!(decided, Some(DeclaredMetaclass::Valid));
    }

    #[test]
    fn get_declared_metaclass_walks_the_gate_chain_in_order() {
        let dynamic = DeclaredMetaclassFacts {
            mc_name: None,
            ..declared_facts()
        };
        let decided = get_declared_metaclass(&dynamic);
        assert_eq!(decided, Some(DeclaredMetaclass::Dynamic));
        let any_var = DeclaredMetaclassFacts {
            sym_is_var: Some(true),
            var_any: Some(true),
            ..declared_facts()
        };
        let decided = get_declared_metaclass(&any_var);
        assert_eq!(decided, Some(DeclaredMetaclass::AnyVar));
        let placeholder = DeclaredMetaclassFacts {
            sym_is_placeholder: Some(true),
            ..declared_facts()
        };
        let decided = get_declared_metaclass(&placeholder);
        assert_eq!(decided, Some(DeclaredMetaclass::Placeholder));
        let tuple_named = DeclaredMetaclassFacts {
            meta_has_tuple_type: Some(true),
            ..declared_facts()
        };
        let decided = get_declared_metaclass(&tuple_named);
        assert_eq!(decided, Some(DeclaredMetaclass::Invalid));
        let not_meta = DeclaredMetaclassFacts {
            meta_is_metaclass: Some(false),
            ..declared_facts()
        };
        let decided = get_declared_metaclass(&not_meta);
        assert_eq!(decided, Some(DeclaredMetaclass::NotMetaclass));
    }

    #[test]
    fn get_declared_metaclass_defers_on_an_unreadable_fact() {
        let unreadable = DeclaredMetaclassFacts {
            sym_is_var: None,
            ..declared_facts()
        };
        assert_eq!(get_declared_metaclass(&unreadable), None);
    }

    #[test]
    fn recalculate_metaclass_leaves_a_plain_class_alone() {
        let facts = RecalculateMetaclassFacts {
            meta_present: true,
            meta_is_enum: Some(false),
            ..Default::default()
        };
        let decided = recalculate_metaclass(&facts);
        assert_eq!(decided, Some(RecalculatedMetaclass::Unchanged));
    }

    #[test]
    fn recalculate_metaclass_installs_abcmeta_for_a_bare_protocol() {
        let facts = RecalculateMetaclassFacts {
            any_protocol_mro: true,
            ..Default::default()
        };
        let decided = recalculate_metaclass(&facts);
        assert_eq!(decided, Some(RecalculatedMetaclass::AbcMeta));
    }

    #[test]
    fn recalculate_metaclass_flags_an_enum_metaclass() {
        let facts = RecalculateMetaclassFacts {
            meta_present: true,
            meta_is_enum: Some(true),
            ..Default::default()
        };
        let decided = recalculate_metaclass(&facts);
        assert_eq!(decided, Some(RecalculatedMetaclass::IsEnum));
        let generic = RecalculateMetaclassFacts {
            type_vars_nonempty: true,
            ..facts
        };
        let decided = recalculate_metaclass(&generic);
        assert_eq!(decided, Some(RecalculatedMetaclass::EnumGenericFail));
    }

    #[test]
    fn recalculate_metaclass_defers_on_an_unreadable_enum_fact() {
        let facts = RecalculateMetaclassFacts {
            meta_present: true,
            ..Default::default()
        };
        assert_eq!(recalculate_metaclass(&facts), None);
    }

    #[test]
    fn analyze_type_with_type_info_puts_a_subscripted_tuple_first() {
        let facts = TypeInfoFacts {
            args_len: 2,
            tuple_type_not_none: true,
            typeddict_type_not_none: true,
            ..Default::default()
        };
        let branch = analyze_type_with_type_info("builtins.tuple", &facts);
        assert_eq!(branch, TypeInfoBranch::Tuple);
    }

    #[test]
    fn analyze_type_with_type_info_binds_a_plain_reference_as_instance() {
        let facts = TypeInfoFacts::default();
        let branch = analyze_type_with_type_info("mod.UserClass", &facts);
        assert_eq!(branch, TypeInfoBranch::Instance);
    }

    #[test]
    fn analyze_type_with_type_info_splits_the_tails_on_the_alias_flag() {
        let plain = TypeInfoFacts {
            tuple_type_not_none: true,
            ..Default::default()
        };
        let branch = analyze_type_with_type_info("mod.Point", &plain);
        assert_eq!(branch, TypeInfoBranch::TupleTail);
        let aliased = TypeInfoFacts {
            tuple_type_not_none: true,
            special_alias_not_none: true,
            ..Default::default()
        };
        let branch = analyze_type_with_type_info("mod.Point", &aliased);
        assert_eq!(branch, TypeInfoBranch::TupleTailAlias);
        let dict = TypeInfoFacts {
            typeddict_type_not_none: true,
            ..Default::default()
        };
        let branch = analyze_type_with_type_info("mod.TD", &dict);
        assert_eq!(branch, TypeInfoBranch::TypedDictTail);
    }

    #[test]
    fn analyze_type_with_type_info_rejects_a_bare_tuple_and_none_type() {
        let bare = TypeInfoFacts::default();
        let branch = analyze_type_with_type_info("builtins.tuple", &bare);
        assert_eq!(branch, TypeInfoBranch::Instance);
        let branch = analyze_type_with_type_info("types.NoneType", &bare);
        assert_eq!(branch, TypeInfoBranch::NoneType);
        let branch = analyze_type_with_type_info("librt.vecs.vec", &bare);
        assert_eq!(branch, TypeInfoBranch::Vec);
    }

    #[test]
    fn analyze_callable_type_dispatches_on_arity_then_arg0() {
        let branch = analyze_callable_type(&CallableFacts::default());
        assert_eq!(branch, CallableBranch::BareCallable);
        let list = CallableFacts {
            arg_count: 2,
            arg0_is_type_list: true,
            ..Default::default()
        };
        let branch = analyze_callable_type(&list);
        assert_eq!(branch, CallableBranch::TypeList);
        let dots = CallableFacts {
            arg_count: 2,
            arg0_is_ellipsis: true,
            ..Default::default()
        };
        let branch = analyze_callable_type(&dots);
        assert_eq!(branch, CallableBranch::Ellipsis);
        let paramspec = CallableFacts {
            arg_count: 2,
            ..Default::default()
        };
        let branch = analyze_callable_type(&paramspec);
        assert_eq!(branch, CallableBranch::ParamSpec);
    }

    #[test]
    fn analyze_callable_type_splits_an_invalid_arity_on_the_option() {
        let invalid = CallableFacts {
            arg_count: 3,
            ..Default::default()
        };
        let branch = analyze_callable_type(&invalid);
        assert_eq!(branch, CallableBranch::InvalidArityAllowed);
        let disallowed = CallableFacts {
            arg_count: 3,
            disallow_any_generics: true,
            ..Default::default()
        };
        let branch = analyze_callable_type(&disallowed);
        assert_eq!(branch, CallableBranch::InvalidArityDisallowed);
    }

    #[test]
    fn analyze_type_guard_arg_matches_both_is_check_families() {
        let one = TypeGuardFacts {
            args_len: 1,
            is_typeis: false,
        };
        let branch = analyze_type_guard_arg("typing.TypeGuard", &one);
        assert_eq!(branch, TypeGuardBranch::Recurse);
        let typeis = TypeGuardFacts {
            args_len: 1,
            is_typeis: true,
        };
        let branch = analyze_type_guard_arg("typing_extensions.TypeIs", &typeis);
        assert_eq!(branch, TypeGuardBranch::Recurse);
    }

    #[test]
    fn analyze_type_guard_arg_rejects_a_bad_arity_or_family() {
        let two = TypeGuardFacts {
            args_len: 2,
            is_typeis: false,
        };
        let branch = analyze_type_guard_arg("typing.TypeGuard", &two);
        assert_eq!(branch, TypeGuardBranch::ArityFail);
        let one = TypeGuardFacts {
            args_len: 1,
            is_typeis: false,
        };
        let branch = analyze_type_guard_arg("typing.TypeIs", &one);
        assert_eq!(branch, TypeGuardBranch::NotAGuard);
        let typeis = TypeGuardFacts {
            args_len: 1,
            is_typeis: true,
        };
        let branch = analyze_type_guard_arg("typing.TypeGuard", &typeis);
        assert_eq!(branch, TypeGuardBranch::NotAGuard);
    }

    #[test]
    fn visit_tuple_type_implicit_reconstructs_an_explicit_tuple() {
        let explicit = ImplicitTupleFacts {
            implicit: false,
            items_len: 3,
            ..Default::default()
        };
        let note = visit_tuple_type_implicit(&explicit);
        assert_eq!(note, ImplicitTupleNote::Reconstruct);
        let allowed = ImplicitTupleFacts {
            implicit: true,
            allow_tuple_literal: true,
            items_len: 3,
        };
        let note = visit_tuple_type_implicit(&allowed);
        assert_eq!(note, ImplicitTupleNote::Reconstruct);
    }

    #[test]
    fn visit_tuple_type_implicit_picks_the_note_by_item_count() {
        let empty = ImplicitTupleFacts {
            implicit: true,
            ..Default::default()
        };
        let note = visit_tuple_type_implicit(&empty);
        assert_eq!(note, ImplicitTupleNote::SuggestEmptyTuple);
        let single = ImplicitTupleFacts {
            implicit: true,
            items_len: 1,
            ..Default::default()
        };
        let note = visit_tuple_type_implicit(&single);
        assert_eq!(note, ImplicitTupleNote::SuggestSpuriousComma);
        let multi = ImplicitTupleFacts {
            implicit: true,
            items_len: 2,
            ..Default::default()
        };
        let note = visit_tuple_type_implicit(&multi);
        assert_eq!(note, ImplicitTupleNote::SuggestTupleOfItems);
    }
}
