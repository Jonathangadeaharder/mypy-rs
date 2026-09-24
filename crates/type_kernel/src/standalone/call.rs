//! Standalone-path public API: call checking.
//!
//! Re-exposes the already-ported call kernel for a caller with no Python
//! interpreter: `check_call` dispatch classification, callable
//! normalization, actual-to-formal argument mapping (star arguments
//! included), the `CallableType` parameter queries, the type-object gates,
//! and the union/overlap helpers `check_call` consults before overload
//! resolution.
//!
//! Provenance is per item: each doc comment names the mypy function the
//! lifted logic comes from (`mypy/checkexpr.py::check_call`,
//! `mypy/argmap.py::map_actuals_to_formals`,
//! `mypy/types.py::CallableType.argument_by_name` and friends). Nothing
//! here registers a seam, changes a hybrid code path, or encodes a wire
//! blob: the lifted kernel functions are called directly on `wire::Type`
//! values, and the crate-private kernel records they return are mirrored by
//! public records defined here (a `pub(crate)` item cannot be re-exported).
//!
//! Deferral is part of the interface. The kernel answers `Option<T>`, where
//! `None` means "the hybrid hands this call back to Python". The standalone
//! path has no Python to hand back to, so `None` is the caller's cue to
//! reject the construct loudly rather than guess (wave-1 rule 6).
//!
//! See docs/plans/2026-09-24-standalone-full-port-wave1.md and ADR-0008.

pub use crate::skeleton_api::{Type, TypeResolver};

use crate::aliases::TypeAliasResolver;
use crate::argmap;
use crate::callable_compat::{self, FormalArgument as KernelFormalArgument};
use crate::checkcall;
use crate::checkcall_typeobj;
use crate::expandtype;

/// `mypy.nodes.ArgKind` (nodes.py:2480-2517) as a typed enum.
///
/// The kernel carries argument kinds as the raw `int(ArgKind.value)` tags
/// mypy's shim passes across the seam. The standalone surface types them,
/// and [`ArgKind::as_i64`] is the only place the tag values are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// `ARG_POS`: a required positional parameter, or a positional actual.
    Pos,
    /// `ARG_OPT`: a positional parameter with a default.
    Opt,
    /// `ARG_STAR`: `*args`.
    Star,
    /// `ARG_NAMED`: a required keyword parameter, or a `name=` actual.
    Named,
    /// `ARG_STAR2`: `**kwargs`.
    Star2,
    /// `ARG_NAMED_OPT`: a keyword parameter with a default.
    NamedOpt,
}

impl ArgKind {
    /// The `mypy.nodes.ArgKind.value` tag the kernel functions take.
    pub const fn as_i64(self) -> i64 {
        match self {
            ArgKind::Pos => 0,
            ArgKind::Opt => 1,
            ArgKind::Star => 2,
            ArgKind::Named => 3,
            ArgKind::Star2 => 4,
            ArgKind::NamedOpt => 5,
        }
    }

    /// Inverse of [`ArgKind::as_i64`]: `None` for a tag outside `0..=5`.
    pub fn from_i64(tag: i64) -> Option<ArgKind> {
        match tag {
            0 => Some(ArgKind::Pos),
            1 => Some(ArgKind::Opt),
            2 => Some(ArgKind::Star),
            3 => Some(ArgKind::Named),
            4 => Some(ArgKind::Star2),
            5 => Some(ArgKind::NamedOpt),
            _ => None,
        }
    }

    /// `ArgKind.is_positional(star)` (nodes.py:2480-2484).
    pub fn is_positional(self, star: bool) -> bool {
        callable_compat::kind_is_positional(self.as_i64(), star)
    }

    /// `ArgKind.is_named(star)` (nodes.py:2486-2490). With `star` set this
    /// is mypy's "named or `**kwargs`" test.
    pub fn is_named(self, star: bool) -> bool {
        callable_compat::kind_is_named(self.as_i64(), star)
    }

    /// `ArgKind.is_star()` (nodes.py:2506-2508).
    pub fn is_star(self) -> bool {
        callable_compat::kind_is_star(self.as_i64())
    }

    /// `ArgKind.is_required()` (nodes.py:2492-2494).
    pub fn is_required(self) -> bool {
        callable_compat::kind_is_required(self.as_i64())
    }

    /// `ArgKind.is_optional()` (nodes.py:2496-2498).
    pub fn is_optional(self) -> bool {
        callable_compat::kind_is_optional(self.as_i64())
    }
}

/// The `check_call` dispatch branch a callee type takes.
///
/// Typed mirror of the `CALL_*` tag set in `crate::checkcall`, which
/// mirrors the `isinstance` chain at the head of
/// `mypy/checkexpr.py::check_call`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallDispatch {
    /// A `CallableType` with no type variables: check it directly.
    PlainCallable,
    /// A `CallableType` with type variables: infer them, then check.
    GenericCallable,
    /// An `Overloaded`: pick an item, then check that item.
    Overloaded,
    /// `Any`, or a function from an unchecked module: the call is unchecked.
    Any,
    /// A union: `check_union_call` checks each item.
    Union,
    /// An `Instance`: routes to `__call__` member access.
    Instance,
    /// A `TypeType`: falls through to member access.
    TypeType,
    /// Anything else: mypy reports it as not callable.
    Other,
}

impl CallDispatch {
    /// Inverse of the kernel's `CALL_*` tags; an unknown tag is
    /// [`CallDispatch::Other`], the branch mypy's chain falls through to.
    pub fn from_i64(tag: i64) -> CallDispatch {
        match tag {
            checkcall::CALL_PLAIN => CallDispatch::PlainCallable,
            checkcall::CALL_WITH_VARS => CallDispatch::GenericCallable,
            checkcall::CALL_OVERLOADED => CallDispatch::Overloaded,
            checkcall::CALL_ANY => CallDispatch::Any,
            checkcall::CALL_UNION => CallDispatch::Union,
            checkcall::CALL_INSTANCE => CallDispatch::Instance,
            checkcall::CALL_TYPE_TYPE => CallDispatch::TypeType,
            _ => CallDispatch::Other,
        }
    }
}

/// The type-object failure arm of `check_callable_call`.
///
/// Typed mirror of the `TYPEOBJ_GATE_*` tags in `crate::checkcall_typeobj`,
/// which mirrors the `if` / `elif` at mypy/checkexpr.py:2941-2969.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeobjGate {
    /// No failure: the call proceeds.
    Pass,
    /// mypy's `CANNOT_INSTANTIATE_PROTOCOL`.
    Protocol,
    /// mypy's `cannot_instantiate_abstract_class`.
    Abstract,
}

impl TypeobjGate {
    fn from_tag(tag: i64) -> TypeobjGate {
        match tag {
            checkcall_typeobj::TYPEOBJ_GATE_PROTOCOL => TypeobjGate::Protocol,
            checkcall_typeobj::TYPEOBJ_GATE_ABSTRACT => TypeobjGate::Abstract,
            _ => TypeobjGate::Pass,
        }
    }
}

/// The callee facts `check_callable_call`'s type-object gate reads.
///
/// mypy reads these live off the `CallableType` and its `type_object()`;
/// the standalone caller supplies them as a record so the gate itself stays
/// a pure decision.
#[derive(Debug, Clone, Copy)]
pub struct TypeobjFacts {
    /// `callee.is_type_obj()`.
    pub is_type_obj: bool,
    /// `callee.type_object().is_protocol`.
    pub is_protocol: bool,
    /// `callee.type_object().is_abstract`.
    pub is_abstract: bool,
    /// `callee.from_type_type`: the `Type[...]` exemption.
    pub from_type_type: bool,
    /// `callee.type_object().fallback_to_any`: the abstract exemption.
    pub fallback_to_any: bool,
}

/// One callable parameter, looked up by name or by position.
///
/// Public mirror of the crate-private `callable_compat::FormalArgument`,
/// which mirrors `mypy.types.FormalArgument` (types.py:252-270). The kernel
/// record cannot be re-exported (`pub(crate)`), so this is the standalone
/// surface's copy; the fields are identical and the two private conversion
/// helpers below are the only points the two meet.
#[derive(Debug, Clone, PartialEq)]
pub struct FormalArgument {
    /// The parameter's name, when it has one.
    pub name: Option<String>,
    /// The positional index, when it can be passed positionally.
    pub pos: Option<usize>,
    /// The parameter's declared type.
    pub typ: Type,
    /// Whether an actual is required (`ArgKind.is_required()`).
    pub required: bool,
}

impl FormalArgument {
    fn from_kernel(arg: &KernelFormalArgument) -> FormalArgument {
        FormalArgument {
            name: arg.name.clone(),
            pos: arg.pos,
            typ: arg.typ.clone(),
            required: arg.required,
        }
    }

    fn to_kernel(&self) -> KernelFormalArgument {
        KernelFormalArgument {
            name: self.name.clone(),
            pos: self.pos,
            typ: self.typ.clone(),
            required: self.required,
        }
    }
}

/// The outcome of [`callable_corresponding_argument`].
///
/// Three-way because the kernel distinguishes "no corresponding parameter"
/// from "producing one needs `meet_types`, which the resolver-free subset
/// cannot decide" (`callable_compat::Defer`).
#[derive(Debug, Clone, PartialEq)]
pub enum CorrespondingArgument {
    /// The corresponding parameter.
    Found(FormalArgument),
    /// Neither the by-name nor the by-position lookup produced one.
    Absent,
    /// Undecidable without `meet_types`; the caller must reject, not guess.
    Undecidable,
}

/// The `CallableType` fields the call kernel reads.
///
/// Public mirror of the crate-private `callable_compat::CallableFields`,
/// built by [`callable_signature`]. Borrowed, so it costs no clone of the
/// argument types.
#[derive(Debug, Clone)]
pub struct CallableSignature<'a> {
    /// `callee.arg_types`.
    pub arg_types: &'a [Type],
    /// `callee.arg_kinds` as raw `ArgKind.value` tags; read them through
    /// [`CallableSignature::arg_kind`] or [`CallableSignature::formal_params`].
    pub arg_kinds: &'a [i64],
    /// `callee.arg_names`.
    pub arg_names: &'a [Option<String>],
    /// `callee.ret_type`.
    pub ret_type: &'a Type,
    /// `callee.is_ellipsis_args`: the `Callable[..., X]` shape.
    pub is_ellipsis_args: bool,
    /// `callee.implicit`: a synthesized signature (e.g. a fallback).
    pub implicit: bool,
    /// `callee.from_concatenate`.
    pub from_concatenate: bool,
    /// `callee.imprecise_arg_kinds`.
    pub imprecise_arg_kinds: bool,
    /// `callee.unpack_kwargs`: a `**kwargs: Unpack[TypedDict]` not yet
    /// expanded (see [`normalize_callable`]).
    pub unpack_kwargs: bool,
    /// `callee.type_guard` payload.
    pub type_guard: Option<&'a Type>,
    /// `callee.type_is` payload.
    pub type_is: Option<&'a Type>,
}

impl<'a> CallableSignature<'a> {
    /// `arg_kinds[i]` as a typed [`ArgKind`]; `None` when the index is out
    /// of range or the tag is not an `ArgKind.value`.
    pub fn arg_kind(&self, i: usize) -> Option<ArgKind> {
        ArgKind::from_i64(*self.arg_kinds.get(i)?)
    }

    /// The callee's parameters as [`FormalParam`] records, the input shape
    /// [`map_actuals_to_formals`] wants. `None` when a kind tag is not an
    /// `ArgKind.value`, so an unreadable signature defers instead of
    /// silently binding as positional.
    pub fn formal_params(&self) -> Option<Vec<FormalParam>> {
        self.arg_kinds
            .iter()
            .zip(self.arg_names.iter())
            .map(|(&kind, name)| Some(FormalParam::new(ArgKind::from_i64(kind)?, name.clone())))
            .collect()
    }
}

/// One actual argument at a call site.
///
/// mypy passes the call site as parallel `arg_kinds` / `arg_names` lists;
/// this record pairs them so the two cannot disagree in length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActualArg {
    /// `arg_kinds[i]`.
    pub kind: ArgKind,
    /// `arg_names[i]`: the `name=` of a keyword actual, else `None`.
    pub name: Option<String>,
}

impl ActualArg {
    /// A positional actual (mypy's `ARG_POS`).
    pub fn positional() -> ActualArg {
        ActualArg {
            kind: ArgKind::Pos,
            name: None,
        }
    }

    /// A keyword actual `name=` (mypy's `ARG_NAMED`).
    pub fn named(name: &str) -> ActualArg {
        ActualArg {
            kind: ArgKind::Named,
            name: Some(name.to_string()),
        }
    }

    /// A `*args` actual (mypy's `ARG_STAR`).
    pub fn star() -> ActualArg {
        ActualArg {
            kind: ArgKind::Star,
            name: None,
        }
    }

    /// A `**kwargs` actual (mypy's `ARG_STAR2`).
    pub fn star2() -> ActualArg {
        ActualArg {
            kind: ArgKind::Star2,
            name: None,
        }
    }
}

/// One formal parameter of a callee.
///
/// The callee-side counterpart of [`ActualArg`]: mypy's `callee.arg_kinds[i]`
/// and `callee.arg_names[i]` as one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormalParam {
    /// `callee.arg_kinds[i]`.
    pub kind: ArgKind,
    /// `callee.arg_names[i]`.
    pub name: Option<String>,
}

impl FormalParam {
    /// A parameter of `kind`; `name` is `None` for a positional-only or a
    /// star parameter, which mypy leaves unnamed at the call boundary.
    pub fn new(kind: ArgKind, name: Option<String>) -> FormalParam {
        FormalParam { kind, name }
    }

    /// A required positional parameter called `name`.
    pub fn positional(name: &str) -> FormalParam {
        FormalParam::new(ArgKind::Pos, Some(name.to_string()))
    }
}

/// The actual-to-formal binding of one call.
///
/// `as_slices()[i]` lists the indices of the actual arguments bound to
/// formal `i`, which is exactly the `formal_to_actual` the rest of the call
/// kernel consumes. A newtype because the forward and the reverse mapping
/// ([`ActualToFormal`]) have identical element types, and passing one where
/// the other is meant would be a silently wrong binding rather than a type
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormalToActual(Vec<Vec<i64>>);

impl FormalToActual {
    /// The mapping as one list of actual indices per formal.
    pub fn as_slices(&self) -> &[Vec<i64>] {
        &self.0
    }

    /// How many formals the mapping covers.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the callee has no formals at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The formal-to-actual binding of one call: `as_slices()[i]` lists the
/// formals actual `i` binds to. The reverse of [`FormalToActual`], and
/// mypy's `actual_to_formal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActualToFormal(Vec<Vec<i64>>);

impl ActualToFormal {
    /// The mapping as one list of formal indices per actual.
    pub fn as_slices(&self) -> &[Vec<i64>] {
        &self.0
    }

    /// How many actuals the mapping covers.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the call site has no actual arguments.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The star-actual type lookup mypy passes to `map_actuals_to_formals` as
/// its `actual_arg_type` callback.
///
/// Substitution (wave-1 rule 4): the hybrid hands the kernel a Python
/// callable and, on the path that avoids one, wire-serialized star-actual
/// types (`argmap::rust_map_actuals_to_formals_with_types`). The standalone
/// caller has neither, so it implements this trait over the types it
/// already holds. `None` means "this actual's type is unavailable", which
/// defers the whole mapping exactly as a missing wire blob does.
pub trait ActualArgType {
    /// The proper type of actual argument `index`, or `None`.
    fn actual_arg_type(&self, index: usize) -> Option<&Type>;
}

/// The `isinstance` chain at the head of `mypy/checkexpr.py::check_call`:
/// which branch a callee type takes.
///
/// Total: every `Type` classifies, and a shape mypy's chain does not name
/// is [`CallDispatch::Other`].
pub fn classify_call(callee: &Type) -> CallDispatch {
    match checkcall::classify_call(callee) {
        Ok(tag) => CallDispatch::from_i64(tag),
        // The kernel classification is `Ok(match ..)` with no fallible step,
        // so this arm is unreachable; it keeps the `Result` honest instead
        // of inventing a branch mypy does not have.
        Err(_) => CallDispatch::Other,
    }
}

/// `CallableType.with_unpacked_kwargs()` then `with_normalized_var_args()`
/// (mypy/types.py:2505-2613), the normalization at the head of
/// `mypy/checkexpr.py::check_callable_call`.
///
/// `None` when the callee is not a `CallableType`, or when a
/// `**kwargs: Unpack[TypedDict]` shape the expansion needs is malformed
/// (mypy asserts there, the kernel defers).
pub fn normalize_callable(callee: &Type) -> Option<Type> {
    checkcall::normalize_callable(callee).ok()
}

/// `TypeType.make_normalized(arg_types[0])` calibration of a type-object
/// callable's return type (step 14 of
/// `mypy/checkexpr.py::check_callable_call`).
///
/// `None` when the callee is not a `CallableType`, or when the argument is
/// a `TypeAliasType`: mypy resolves the alias target before normalizing and
/// the kernel has no target to resolve, so the calibration defers. Unlike
/// [`check_callable_call_tail`] this applies the calibration unconditionally;
/// the `is_type_obj()` gate is the caller's.
pub fn calibrate_type_obj_return(callee: &Type, arg_type: &Type) -> Option<Type> {
    if matches!(arg_type, Type::TypeAliasType { .. }) {
        return None;
    }
    let new_ret = expandtype::make_type_normalized(arg_type.clone(), false);
    let mut base = checkcall::callable_base(callee).ok()?;
    base.ret_type = Box::new(new_ret);
    Some(base.into_type())
}

/// The `check_callable_call` tail that runs after argument binding
/// (mypy/checkexpr.py:2548-2627): the type-object return calibration, gated
/// on `is_type_obj()` and on exactly one actual argument.
///
/// `None` means the whole tail is undecidable here (not a type object, more
/// than one argument, an alias component, or a target whose force-fallback
/// walk the kernel cannot follow), so the caller must not assume the
/// callee is unchanged.
pub fn check_callable_call_tail(callee: &Type, arg_types: &[Type]) -> Option<Type> {
    checkcall::check_callable_call_tail(callee, arg_types)
}

/// `CallableType.is_type_obj()` (mypy/types.py:2343-2346): the fallback's
/// class is a metaclass and the return type is not `UninhabitedType`.
///
/// `None` when the callee is not a `CallableType`, its fallback is not an
/// `Instance`, or the resolver has no snapshot for the fallback (so
/// `is_metaclass()` cannot be decided).
pub fn is_type_obj(callee: &Type, resolver: &TypeResolver) -> Option<bool> {
    callable_compat::is_type_obj(callee, resolver)
}

/// The type-object failure arm of `mypy/checkexpr.py::check_callable_call`
/// (checkexpr.py:2941-2969), in mypy's short-circuit order: a protocol
/// type object fails first, then an abstract one that neither
/// `from_type_type` nor `fallback_to_any` exempts.
pub fn classify_typeobj_gate(facts: &TypeobjFacts) -> TypeobjGate {
    TypeobjGate::from_tag(checkcall_typeobj::classify_typeobj_gate(
        facts.is_type_obj,
        facts.is_protocol,
        facts.is_abstract,
        facts.from_type_type,
        facts.fallback_to_any,
    ))
}

/// The `CallableType` fields the call kernel reads. `None` when `callee` is
/// not a `CallableType` (a `Parameters`, an `Instance`, ...).
pub fn callable_signature(callee: &Type) -> Option<CallableSignature<'_>> {
    let fields = callable_compat::callable_fields(callee)?;
    Some(CallableSignature {
        arg_types: fields.arg_types,
        arg_kinds: fields.arg_kinds,
        arg_names: fields.arg_names,
        ret_type: fields.ret_type,
        is_ellipsis_args: fields.is_ellipsis_args,
        implicit: fields.implicit,
        from_concatenate: fields.from_concatenate,
        imprecise_arg_kinds: fields.imprecise_arg_kinds,
        unpack_kwargs: fields.unpack_kwargs,
        type_guard: fields.type_guard,
        type_is: fields.type_is,
    })
}

/// `CallableType.var_arg` (mypy/types.py:2314-2320): the first `*args`
/// parameter, or `None` when the signature has none.
pub fn var_arg(sig: &CallableSignature<'_>) -> Option<FormalArgument> {
    callable_compat::var_arg(sig.arg_types, sig.arg_kinds)
        .as_ref()
        .map(FormalArgument::from_kernel)
}

/// `CallableType.kw_arg` (mypy/types.py:2322-2327): the first `**kwargs`
/// parameter, or `None` when the signature has none.
pub fn kw_arg(sig: &CallableSignature<'_>) -> Option<FormalArgument> {
    callable_compat::kw_arg(sig.arg_types, sig.arg_kinds)
        .as_ref()
        .map(FormalArgument::from_kernel)
}

/// `CallableType.formal_arguments` (mypy/types.py:2398-2419): the non-star
/// parameters in order, with `pos` cleared from the first named-or-star
/// parameter onwards (mypy's `done_with_positional`).
pub fn formal_arguments(sig: &CallableSignature<'_>) -> Vec<FormalArgument> {
    callable_compat::formal_arguments(sig.arg_types, sig.arg_kinds, sig.arg_names)
        .iter()
        .map(FormalArgument::from_kernel)
        .collect()
}

/// `CallableType.argument_by_name` (mypy/types.py:2421-2436): the parameter
/// called `name`, else the one synthesized from `**kwargs`, else `None`.
pub fn argument_by_name(sig: &CallableSignature<'_>, name: &str) -> Option<FormalArgument> {
    callable_compat::argument_by_name(sig.arg_types, sig.arg_kinds, sig.arg_names, name)
        .as_ref()
        .map(FormalArgument::from_kernel)
}

/// `CallableType.argument_by_position` (mypy/types.py:2438-2451): the
/// parameter at `position` when it accepts positional arguments, else the
/// one synthesized from `*args`, else `None`.
pub fn argument_by_position(
    sig: &CallableSignature<'_>,
    position: usize,
) -> Option<FormalArgument> {
    callable_compat::argument_by_position(sig.arg_types, sig.arg_kinds, sig.arg_names, position)
        .as_ref()
        .map(FormalArgument::from_kernel)
}

/// `CallableType.try_synthesizing_arg_from_kwarg` (mypy/types.py:2453-2458):
/// any name maps to the `**kwargs` type when the signature has one.
pub fn synthesize_arg_from_kwarg(
    sig: &CallableSignature<'_>,
    name: Option<String>,
) -> Option<FormalArgument> {
    callable_compat::try_synthesizing_arg_from_kwarg(sig.arg_types, sig.arg_kinds, name)
        .as_ref()
        .map(FormalArgument::from_kernel)
}

/// `CallableType.try_synthesizing_arg_from_vararg` (mypy/types.py:2460-2465):
/// any out-of-range position maps to the `*args` type when the signature has
/// one.
pub fn synthesize_arg_from_vararg(
    sig: &CallableSignature<'_>,
    position: Option<usize>,
) -> Option<FormalArgument> {
    callable_compat::try_synthesizing_arg_from_vararg(sig.arg_types, sig.arg_kinds, position)
        .as_ref()
        .map(FormalArgument::from_kernel)
}

/// `mypy.typeops.callable_corresponding_argument` (typeops.py:1170-1204):
/// the parameter of `sig` that corresponds to `model`, merging the by-name
/// and by-position lookups.
pub fn callable_corresponding_argument(
    sig: &CallableSignature<'_>,
    model: &FormalArgument,
) -> CorrespondingArgument {
    let kernel_model = model.to_kernel();
    match callable_compat::callable_corresponding_argument(
        sig.arg_types,
        sig.arg_kinds,
        sig.arg_names,
        &kernel_model,
    ) {
        Ok(Some(arg)) => CorrespondingArgument::Found(FormalArgument::from_kernel(&arg)),
        Ok(None) => CorrespondingArgument::Absent,
        Err(_) => CorrespondingArgument::Undecidable,
    }
}

/// `mypy/argmap.py::map_actuals_to_formals` (argmap.py:27-122): which actual
/// argument binds to which formal parameter.
///
/// `None` is the kernel's deferral, and for a standalone caller it has two
/// distinct causes worth knowing: the call has an `*args` or `**kwargs`
/// actual (use [`map_actuals_to_formals_with_types`], which can decide it),
/// or a keyword actual carries no name (a producer bug mypy asserts on).
///
/// Binding is lossy by design, exactly as in mypy: a positional actual past
/// the last formal is dropped (the arity error is
/// `mypy/checkexpr.py::check_argument_count`'s job, not the mapper's), and
/// an unmatched keyword actual routes to `**kwargs` when the callee has one.
pub fn map_actuals_to_formals(
    actual: &[ActualArg],
    formal: &[FormalParam],
) -> Option<FormalToActual> {
    let (actual_kinds, actual_names) = split_actual(actual);
    let (formal_kinds, formal_names) = split_formal(formal);
    let mapped = argmap::rust_map_actuals_to_formals(
        actual_kinds,
        actual_names,
        formal_kinds,
        formal_names,
    )?;
    Some(FormalToActual(mapped))
}

/// `mypy/argmap.py::map_formals_to_actuals` (argmap.py:167-183): the reverse
/// of [`map_actuals_to_formals`], indexed by actual argument. Defers on
/// exactly the same inputs.
pub fn map_formals_to_actuals(
    actual: &[ActualArg],
    formal: &[FormalParam],
) -> Option<ActualToFormal> {
    let (actual_kinds, actual_names) = split_actual(actual);
    let (formal_kinds, formal_names) = split_formal(formal);
    let mapped = argmap::rust_map_formals_to_actuals(
        actual_kinds,
        actual_names,
        formal_kinds,
        formal_names,
    )?;
    Some(ActualToFormal(mapped))
}

/// [`map_actuals_to_formals`] for a call with `*args` / `**kwargs` actuals:
/// the star arms of `mypy/argmap.py::map_actuals_to_formals`
/// (argmap.py:96-163), reading each star actual's type through
/// [`ActualArgType`] instead of mypy's `actual_arg_type` callback.
///
/// A call with no star actual delegates to [`map_actuals_to_formals`], so
/// the common path is the kernel's own. The star arms restate the logic of
/// `argmap::rust_map_actuals_to_formals_with_types`, which is identical but
/// reads its star types from wire blobs the standalone path must not
/// decode: a `*args` of tuple type binds one formal per item, any other
/// `*args` binds every remaining positional formal, a `**kwargs` of
/// TypedDict type binds by key name, and an unknown `**kwargs` is treated as
/// filling every still-unbound formal (mypy's ambiguous-kwargs pass).
pub fn map_actuals_to_formals_with_types(
    actual: &[ActualArg],
    formal: &[FormalParam],
    actual_types: &dyn ActualArgType,
) -> Option<FormalToActual> {
    if !actual.iter().any(|arg| arg.kind.is_star()) {
        return map_actuals_to_formals(actual, formal);
    }
    let nformals = formal.len();
    let mut mapped: Vec<Vec<i64>> = vec![Vec::new(); nformals];
    let mut ambiguous_kwargs: Vec<i64> = Vec::new();
    let mut fi = 0usize;
    for (ai, arg) in actual.iter().enumerate() {
        let ai64 = ai as i64;
        match arg.kind {
            ArgKind::Pos => {
                if fi < nformals {
                    let kind = formal[fi].kind;
                    if !kind.is_star() {
                        mapped[fi].push(ai64);
                        fi += 1;
                    } else if kind == ArgKind::Star {
                        mapped[fi].push(ai64);
                    }
                }
            }
            ArgKind::Star => {
                let actualt = actual_types.actual_arg_type(ai)?;
                if let Type::TupleType { items, .. } = actualt {
                    for _ in 0..items.len() {
                        if fi >= nformals {
                            break;
                        }
                        let kind = formal[fi].kind;
                        if kind == ArgKind::Star2 {
                            break;
                        }
                        mapped[fi].push(ai64);
                        if kind != ArgKind::Star {
                            fi += 1;
                        }
                    }
                } else {
                    while fi < nformals {
                        let kind = formal[fi].kind;
                        if kind.is_named(true) {
                            break;
                        }
                        mapped[fi].push(ai64);
                        if kind == ArgKind::Star {
                            break;
                        }
                        fi += 1;
                    }
                }
            }
            ArgKind::Named | ArgKind::NamedOpt => {
                let name = arg.name.as_deref()?;
                let by_name = formal.iter().position(|p| p.name.as_deref() == Some(name));
                match by_name {
                    Some(idx) if formal[idx].kind != ArgKind::Star => mapped[idx].push(ai64),
                    Some(_) | None => {
                        if let Some(s2) = formal.iter().position(|p| p.kind == ArgKind::Star2) {
                            mapped[s2].push(ai64);
                        }
                    }
                }
            }
            ArgKind::Star2 => {
                let actualt = actual_types.actual_arg_type(ai)?;
                if let Type::TypedDictType { items, .. } = actualt {
                    for (key, _) in items {
                        let by_name = formal
                            .iter()
                            .position(|p| p.name.as_deref() == Some(key.as_str()));
                        match by_name {
                            Some(idx) => mapped[idx].push(ai64),
                            None => {
                                let s2 = formal.iter().position(|p| p.kind == ArgKind::Star2);
                                if let Some(s2) = s2 {
                                    mapped[s2].push(ai64);
                                }
                            }
                        }
                    }
                } else {
                    ambiguous_kwargs.push(ai64);
                }
            }
            // mypy asserts `ARG_OPT` is unreachable among actuals.
            ArgKind::Opt => return None,
        }
    }
    if !ambiguous_kwargs.is_empty() {
        // mypy computes the fill target set once, before any ambiguous
        // actual is bound; recomputing it per actual would see the
        // previous fill and drop later formals.
        let unbound = unbound_formals(&mapped, actual, formal);
        for &ai64 in &ambiguous_kwargs {
            for &fi in &unbound {
                mapped[fi].push(ai64);
            }
        }
    }
    Some(FormalToActual(mapped))
}

/// The reverse of [`map_actuals_to_formals_with_types`], indexed by actual
/// argument: mypy's `actual_to_formal` for a call with star actuals.
pub fn map_formals_to_actuals_with_types(
    actual: &[ActualArg],
    formal: &[FormalParam],
    actual_types: &dyn ActualArgType,
) -> Option<ActualToFormal> {
    let forward = map_actuals_to_formals_with_types(actual, formal, actual_types)?;
    Some(ActualToFormal(reverse_mapping(
        forward.as_slices(),
        actual.len(),
    )))
}

/// mypy's ambiguous-kwargs pass (argmap.py:142-163): the formals an unknown
/// `**kwargs` is assumed to fill. A named formal counts when nothing bound
/// to it yet, or only a `*args` actual did; `*args` formals never count;
/// the callee's own `**kwargs` always does.
fn unbound_formals(
    mapped: &[Vec<i64>],
    actual: &[ActualArg],
    formal: &[FormalParam],
) -> Vec<usize> {
    (0..formal.len())
        .filter(|&i| {
            let named = formal[i].name.as_deref().is_some_and(|n| !n.is_empty());
            let star = formal[i].kind == ArgKind::Star;
            let star2 = formal[i].kind == ArgKind::Star2;
            let first = mapped[i].first().and_then(|&a| actual.get(a as usize));
            let first_is_star = first.is_some_and(|arg| arg.kind == ArgKind::Star);
            let unbound = mapped[i].is_empty() || first_is_star;
            (named && unbound && !star) || star2
        })
        .collect()
}

/// Reverse a `formal_to_actual` mapping into `actual_to_formal`, the loop
/// `argmap::rust_map_formals_to_actuals` runs inline.
fn reverse_mapping(formal_to_actual: &[Vec<i64>], n_actuals: usize) -> Vec<Vec<i64>> {
    let mut actual_to_formal: Vec<Vec<i64>> = vec![Vec::new(); n_actuals];
    for (formal, actuals) in formal_to_actual.iter().enumerate() {
        for &actual in actuals {
            if let Some(slot) = actual_to_formal.get_mut(actual as usize) {
                slot.push(formal as i64);
            }
        }
    }
    actual_to_formal
}

/// `ExpressionChecker.real_union` (mypy/checkexpr.py:4541-4550): whether a
/// type is a union with more than one relevant item, which is what makes
/// `check_call` dispatch per union item. With `strict_optional` off,
/// `NoneType` items do not count.
///
/// `None` when an alias in the type cannot be expanded from the resolver's
/// snapshots.
pub fn real_union(typ: &Type, strict_optional: bool, resolver: &TypeResolver) -> Option<bool> {
    let aliases = alias_view(resolver);
    checkcall::real_union(typ, strict_optional, &aliases)
}

/// `ExpressionChecker.possible_none_type_var_overlap`
/// (mypy/checkexpr.py:3348): the heuristic that forces union math during
/// overload resolution. True when some actual is a union containing
/// `NoneType` and one plausible target has a `NoneType` formal where
/// another has a `TypeVarType` formal at the same position.
///
/// `None` when an alias in either input cannot be expanded.
pub fn possible_none_type_var_overlap(
    arg_types: &[Type],
    plausible_targets: &[Type],
    resolver: &TypeResolver,
) -> Option<bool> {
    let aliases = alias_view(resolver);
    checkcall::possible_none_type_var_overlap(arg_types, plausible_targets, &aliases)
}

/// Whether a callable carries an `Unpack[...]` in any argument or in its
/// return type: the gate `mypy/subtypes.py::is_callable_compatible` and the
/// overload paths use to refuse a variadic shape they cannot compare.
pub fn any_unpack_anywhere(typ: &Type) -> bool {
    callable_compat::any_unpack_anywhere(typ)
}

/// The resolver's alias snapshots as the lookup the expansion helpers take.
/// Zero-copy while no alias is inserted; mirrors the in-tree construction in
/// `callable_compat::callables_compatible_with_ignore_return`.
fn alias_view(resolver: &TypeResolver) -> TypeAliasResolver {
    match resolver.aliases() {
        Some(shared) => TypeAliasResolver::from_shared_view(shared),
        None => TypeAliasResolver::new(),
    }
}

/// Split a call site into the two parallel lists the kernel mapper takes.
fn split_actual(actual: &[ActualArg]) -> (Vec<i64>, Vec<Option<String>>) {
    let kinds = actual.iter().map(|arg| arg.kind.as_i64()).collect();
    let names = actual.iter().map(|arg| arg.name.clone()).collect();
    (kinds, names)
}

/// Split a callee's parameters into the two parallel lists the kernel mapper
/// takes.
fn split_formal(formal: &[FormalParam]) -> (Vec<i64>, Vec<Option<String>>) {
    let kinds = formal.iter().map(|param| param.kind.as_i64()).collect();
    let names = formal.iter().map(|param| param.name.clone()).collect();
    (kinds, names)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::skeleton_api::TypeInfoSnapshot;

    fn any_type() -> Type {
        Type::AnyType {
            type_of_any: 0,
            source_any: None,
            missing_import_name: None,
        }
    }

    fn instance(fullname: &str) -> Type {
        Type::Instance {
            type_ref: fullname.to_string(),
            args: Vec::new(),
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn none_type() -> Type {
        Type::NoneType
    }

    fn union_of(items: Vec<Type>) -> Type {
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

    fn type_var() -> Type {
        Type::TypeVarType {
            name: "T".to_string(),
            fullname: "mod.T".to_string(),
            raw_id: 1,
            namespace: "mod".to_string(),
            values: Vec::new(),
            upper_bound: Box::new(any_type()),
            default: Box::new(any_type()),
            variance: 0,
            meta_level: 0,
        }
    }

    fn tuple_of(items: Vec<Type>) -> Type {
        Type::TupleType {
            partial_fallback: Box::new(instance("builtins.tuple")),
            items,
            implicit: false,
        }
    }

    fn unpack(typ: Type) -> Type {
        Type::UnpackType {
            typ: Box::new(typ),
            from_star_syntax: false,
        }
    }

    fn alias() -> Type {
        Type::TypeAliasType {
            args: Vec::new(),
            type_ref: "mod.A".to_string(),
            is_recursive: false,
        }
    }

    fn typed_dict(keys: &[&str]) -> Type {
        let mut items: Vec<(String, Type)> = Vec::new();
        let mut required: HashSet<String> = HashSet::new();
        for key in keys {
            items.push(((*key).to_string(), any_type()));
            required.insert((*key).to_string());
        }
        Type::TypedDictType {
            fallback: Box::new(instance("builtins.dict")),
            items,
            required_keys: required,
            readonly_keys: HashSet::new(),
            is_closed: true,
        }
    }

    fn callable_shaped(
        fallback: &str,
        unpack_kwargs: bool,
        variables: Vec<Type>,
        ret_type: Type,
        args: Vec<(Type, ArgKind, Option<String>)>,
    ) -> Type {
        let mut arg_types = Vec::with_capacity(args.len());
        let mut arg_kinds = Vec::with_capacity(args.len());
        let mut arg_names = Vec::with_capacity(args.len());
        for (typ, kind, name) in args {
            arg_types.push(typ);
            arg_kinds.push(kind.as_i64());
            arg_names.push(name);
        }
        Type::CallableType {
            fallback: Box::new(instance(fallback)),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: false,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs,
            from_type_type: false,
            arg_types,
            arg_kinds,
            arg_names,
            ret_type: Box::new(ret_type),
            name: None,
            variables,
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    /// A plain `CallableType` over `builtins.function` returning `Any`.
    fn callable(args: Vec<(Type, ArgKind, Option<String>)>) -> Type {
        callable_shaped("builtins.function", false, Vec::new(), any_type(), args)
    }

    /// A `CallableType` with no parameters over `fallback`.
    fn with_fallback(fallback: &str, ret_type: Type) -> Type {
        callable_shaped(fallback, false, Vec::new(), ret_type, Vec::new())
    }

    /// A type object: `builtins.type` as both fallback and instance type,
    /// one positional formal.
    fn type_obj(ret_type: Type) -> Type {
        let mut callee = callable_shaped(
            "builtins.type",
            false,
            Vec::new(),
            ret_type,
            vec![(instance("builtins.str"), ArgKind::Pos, None)],
        );
        if let Type::CallableType { instance_type, .. } = &mut callee {
            *instance_type = Some(Box::new(instance("builtins.type")));
        }
        callee
    }

    /// A resolver holding one metaclass: `mod.Meta` has `builtins.type` in
    /// its precomputed base set, which is what `is_metaclass()` reads.
    fn resolver_with_metaclass() -> TypeResolver {
        let mut resolver = TypeResolver::new();
        let mut snap = TypeInfoSnapshot::default();
        snap.fullname = "mod.Meta".to_string();
        snap.has_base.insert("builtins.type".to_string());
        resolver.insert("mod.Meta".to_string(), snap);
        resolver
    }

    /// A resolver holding `builtins.function`, which is not a metaclass.
    fn resolver_with_function() -> TypeResolver {
        let mut resolver = TypeResolver::new();
        let mut snap = TypeInfoSnapshot::default();
        snap.fullname = "builtins.function".to_string();
        resolver.insert("builtins.function".to_string(), snap);
        resolver
    }

    /// The star-actual types a call site holds, indexed by actual position.
    struct StarTypes(Vec<Option<Type>>);

    impl ActualArgType for StarTypes {
        fn actual_arg_type(&self, index: usize) -> Option<&Type> {
            self.0.get(index)?.as_ref()
        }
    }

    fn positional(count: usize) -> Vec<ActualArg> {
        vec![ActualArg::positional(); count]
    }

    fn formals(names: &[&str]) -> Vec<FormalParam> {
        names.iter().map(|n| FormalParam::positional(n)).collect()
    }

    fn star_formal() -> FormalParam {
        FormalParam::new(ArgKind::Star, None)
    }

    fn star2_formal() -> FormalParam {
        FormalParam::new(ArgKind::Star2, None)
    }

    fn signature(callee: &Type) -> CallableSignature<'_> {
        callable_signature(callee).expect("the callee is a CallableType in these tests")
    }

    fn expect_type(typ: Option<Type>) -> Type {
        typ.expect("the lifted operation decides")
    }

    fn expect_mapped(mapped: Option<FormalToActual>) -> FormalToActual {
        mapped.expect("the mapping decides")
    }

    fn expect_reverse(mapped: Option<ActualToFormal>) -> ActualToFormal {
        mapped.expect("the reverse mapping decides")
    }

    fn expect_found(arg: Option<FormalArgument>) -> FormalArgument {
        arg.expect("the lookup finds a parameter")
    }

    #[test]
    fn arg_kind_round_trips_its_tags() {
        let kinds = [
            ArgKind::Pos,
            ArgKind::Opt,
            ArgKind::Star,
            ArgKind::Named,
            ArgKind::Star2,
            ArgKind::NamedOpt,
        ];
        for (tag, kind) in kinds.iter().enumerate() {
            assert_eq!(kind.as_i64(), tag as i64);
            assert_eq!(ArgKind::from_i64(tag as i64), Some(*kind));
        }
        assert_eq!(ArgKind::from_i64(6), None);
    }

    #[test]
    fn arg_kind_predicates_match_mypy() {
        assert!(ArgKind::Pos.is_positional(false));
        assert!(ArgKind::Opt.is_positional(false));
        assert!(ArgKind::Star.is_positional(true));
        assert!(!ArgKind::Star.is_positional(false));
        assert!(ArgKind::Star2.is_named(true));
        assert!(!ArgKind::Star2.is_named(false));
        assert!(ArgKind::Named.is_named(false));
        assert!(ArgKind::Pos.is_required());
        assert!(ArgKind::Named.is_required());
        assert!(ArgKind::Opt.is_optional());
        assert!(ArgKind::NamedOpt.is_optional());
        assert!(!ArgKind::NamedOpt.is_required());
        assert!(ArgKind::Star.is_star());
        assert!(ArgKind::Star2.is_star());
        assert!(!ArgKind::Pos.is_star());
    }

    #[test]
    fn classify_call_covers_the_dispatch_chain() {
        let plain = callable(Vec::new());
        assert_eq!(classify_call(&plain), CallDispatch::PlainCallable);
        let generic = callable_shaped(
            "builtins.function",
            false,
            vec![type_var()],
            any_type(),
            Vec::new(),
        );
        assert_eq!(classify_call(&generic), CallDispatch::GenericCallable);
        let overloaded = Type::Overloaded {
            items: vec![callable(Vec::new())],
        };
        assert_eq!(classify_call(&overloaded), CallDispatch::Overloaded);
        assert_eq!(classify_call(&any_type()), CallDispatch::Any);
        let union = union_of(vec![instance("builtins.int"), none_type()]);
        assert_eq!(classify_call(&union), CallDispatch::Union);
        assert_eq!(classify_call(&instance("mod.C")), CallDispatch::Instance);
        let type_type = Type::TypeType {
            item: Box::new(instance("mod.C")),
            is_type_form: false,
        };
        assert_eq!(classify_call(&type_type), CallDispatch::TypeType);
        assert_eq!(classify_call(&none_type()), CallDispatch::Other);
    }

    #[test]
    fn normalize_callable_expands_unpacked_kwargs() {
        let kwargs = typed_dict(&["a", "b"]);
        let callee = callable_shaped(
            "builtins.function",
            true,
            Vec::new(),
            any_type(),
            vec![(kwargs, ArgKind::Star2, None)],
        );
        let normalized = expect_type(normalize_callable(&callee));
        let sig = signature(&normalized);
        assert_eq!(sig.arg_types.len(), 2);
        assert_eq!(sig.arg_kinds.to_vec(), vec![ArgKind::Named.as_i64(); 2]);
        assert_eq!(sig.arg_names[0].as_deref(), Some("a"));
        assert_eq!(sig.arg_names[1].as_deref(), Some("b"));
        assert!(!sig.unpack_kwargs);
    }

    #[test]
    fn normalize_callable_expands_a_var_arg_tuple() {
        let unpacked = unpack(tuple_of(vec![any_type(), any_type()]));
        let callee = callable(vec![(unpacked, ArgKind::Star, None)]);
        let normalized = expect_type(normalize_callable(&callee));
        let sig = signature(&normalized);
        assert_eq!(sig.arg_types.len(), 2);
        assert_eq!(sig.arg_kinds.to_vec(), vec![ArgKind::Pos.as_i64(); 2]);
    }

    #[test]
    fn normalize_callable_rejects_a_non_callable() {
        assert_eq!(normalize_callable(&instance("builtins.int")), None);
    }

    #[test]
    fn calibrate_type_obj_return_wraps_the_argument() {
        let arg = instance("builtins.str");
        let callee = callable(vec![(arg.clone(), ArgKind::Pos, None)]);
        let out = expect_type(calibrate_type_obj_return(&callee, &arg));
        let Type::CallableType { ret_type, .. } = out else {
            panic!("expected a CallableType");
        };
        assert_eq!(
            *ret_type,
            Type::TypeType {
                item: Box::new(arg),
                is_type_form: false,
            }
        );
    }

    #[test]
    fn calibrate_type_obj_return_distributes_a_union() {
        let arg = union_of(vec![instance("builtins.int"), instance("builtins.str")]);
        let callee = callable(vec![(any_type(), ArgKind::Pos, None)]);
        let out = expect_type(calibrate_type_obj_return(&callee, &arg));
        let Type::CallableType { ret_type, .. } = out else {
            panic!("expected a CallableType");
        };
        assert_eq!(
            *ret_type,
            union_of(vec![
                Type::TypeType {
                    item: Box::new(instance("builtins.int")),
                    is_type_form: false,
                },
                Type::TypeType {
                    item: Box::new(instance("builtins.str")),
                    is_type_form: false,
                },
            ])
        );
    }

    #[test]
    fn calibrate_type_obj_return_defers_on_an_alias_argument() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, None)]);
        assert_eq!(calibrate_type_obj_return(&callee, &alias()), None);
    }

    #[test]
    fn calibrate_type_obj_return_rejects_a_non_callable_callee() {
        let callee = instance("builtins.int");
        let arg = instance("builtins.str");
        assert_eq!(calibrate_type_obj_return(&callee, &arg), None);
    }

    #[test]
    fn check_callable_call_tail_calibrates_a_type_object() {
        let callee = type_obj(any_type());
        let args = [instance("builtins.str")];
        let out = expect_type(check_callable_call_tail(&callee, &args));
        let Type::CallableType { ret_type, .. } = out else {
            panic!("expected a CallableType");
        };
        assert_eq!(
            *ret_type,
            Type::TypeType {
                item: Box::new(instance("builtins.str")),
                is_type_form: false,
            }
        );
    }

    #[test]
    fn check_callable_call_tail_defers_for_a_plain_callable() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, None)]);
        let args = [instance("builtins.str")];
        assert_eq!(check_callable_call_tail(&callee, &args), None);
    }

    #[test]
    fn check_callable_call_tail_defers_on_two_arguments() {
        let callee = type_obj(any_type());
        let args = [instance("builtins.str"), instance("builtins.int")];
        assert_eq!(check_callable_call_tail(&callee, &args), None);
    }

    #[test]
    fn is_type_obj_true_for_a_metaclass_fallback() {
        let callee = with_fallback("mod.Meta", instance("builtins.str"));
        let resolver = resolver_with_metaclass();
        assert_eq!(is_type_obj(&callee, &resolver), Some(true));
    }

    #[test]
    fn is_type_obj_false_for_a_plain_function_fallback() {
        let callee = with_fallback("builtins.function", instance("builtins.str"));
        let resolver = resolver_with_function();
        assert_eq!(is_type_obj(&callee, &resolver), Some(false));
    }

    #[test]
    fn is_type_obj_defers_without_the_snapshot() {
        let callee = with_fallback("mod.Meta", instance("builtins.str"));
        let resolver = TypeResolver::new();
        assert_eq!(is_type_obj(&callee, &resolver), None);
    }

    #[test]
    fn classify_typeobj_gate_fires_the_protocol_arm() {
        let facts = TypeobjFacts {
            is_type_obj: true,
            is_protocol: true,
            is_abstract: false,
            from_type_type: false,
            fallback_to_any: false,
        };
        assert_eq!(classify_typeobj_gate(&facts), TypeobjGate::Protocol);
    }

    #[test]
    fn classify_typeobj_gate_fires_the_abstract_arm() {
        let facts = TypeobjFacts {
            is_type_obj: true,
            is_protocol: false,
            is_abstract: true,
            from_type_type: false,
            fallback_to_any: false,
        };
        assert_eq!(classify_typeobj_gate(&facts), TypeobjGate::Abstract);
    }

    #[test]
    fn classify_typeobj_gate_exemptions_pass() {
        let mut facts = TypeobjFacts {
            is_type_obj: true,
            is_protocol: true,
            is_abstract: true,
            from_type_type: true,
            fallback_to_any: false,
        };
        assert_eq!(classify_typeobj_gate(&facts), TypeobjGate::Pass);
        facts.from_type_type = false;
        facts.fallback_to_any = true;
        assert_eq!(classify_typeobj_gate(&facts), TypeobjGate::Pass);
        facts.is_type_obj = false;
        facts.fallback_to_any = false;
        assert_eq!(classify_typeobj_gate(&facts), TypeobjGate::Pass);
    }

    #[test]
    fn callable_signature_reads_a_callable() {
        let callee = callable(vec![(instance("builtins.int"), ArgKind::Pos, None)]);
        let sig = signature(&callee);
        assert_eq!(sig.arg_types.len(), 1);
        assert_eq!(sig.arg_kind(0), Some(ArgKind::Pos));
        assert_eq!(sig.arg_kind(1), None);
        assert!(!sig.is_ellipsis_args);
        assert!(!sig.unpack_kwargs);
        assert_eq!(sig.type_guard, None);
    }

    #[test]
    fn callable_signature_rejects_an_instance() {
        assert!(callable_signature(&instance("mod.C")).is_none());
    }

    #[test]
    fn callable_signature_types_the_formal_params() {
        let callee = callable(vec![
            (
                instance("builtins.int"),
                ArgKind::Pos,
                Some("a".to_string()),
            ),
            (any_type(), ArgKind::Star2, None),
        ]);
        let sig = signature(&callee);
        let params = sig.formal_params().expect("the kinds are ArgKind tags");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], FormalParam::positional("a"));
        assert_eq!(params[1].kind, ArgKind::Star2);
        assert_eq!(params[1].name, None);
    }

    #[test]
    fn var_arg_and_kw_arg_find_the_star_formals() {
        let callee = callable(vec![
            (
                instance("builtins.int"),
                ArgKind::Pos,
                Some("a".to_string()),
            ),
            (any_type(), ArgKind::Star, None),
            (any_type(), ArgKind::Star2, None),
        ]);
        let sig = signature(&callee);
        let var = expect_found(var_arg(&sig));
        assert_eq!(var.pos, Some(1));
        assert!(!var.required);
        let kw = expect_found(kw_arg(&sig));
        assert_eq!(kw.pos, Some(2));
    }

    #[test]
    fn var_arg_absent_without_a_star_formal() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, Some("a".to_string()))]);
        let sig = signature(&callee);
        assert_eq!(var_arg(&sig), None);
        assert_eq!(kw_arg(&sig), None);
    }

    #[test]
    fn formal_arguments_skips_star_formals() {
        let callee = callable(vec![
            (
                instance("builtins.int"),
                ArgKind::Pos,
                Some("a".to_string()),
            ),
            (any_type(), ArgKind::Star, None),
            (any_type(), ArgKind::Star2, None),
        ]);
        let sig = signature(&callee);
        let args = formal_arguments(&sig);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name.as_deref(), Some("a"));
        assert_eq!(args[0].pos, Some(0));
        assert!(args[0].required);
    }

    #[test]
    fn formal_arguments_end_the_positional_section_at_a_named() {
        let callee = callable(vec![
            (any_type(), ArgKind::Pos, Some("a".to_string())),
            (any_type(), ArgKind::Named, Some("b".to_string())),
            (any_type(), ArgKind::Opt, Some("c".to_string())),
        ]);
        let sig = signature(&callee);
        let args = formal_arguments(&sig);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0].pos, Some(0));
        assert_eq!(args[1].pos, None);
        assert!(args[1].required);
        assert_eq!(args[2].pos, None);
        assert!(!args[2].required);
    }

    #[test]
    fn argument_by_name_matches_a_formal() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, Some("a".to_string()))]);
        let sig = signature(&callee);
        let found = expect_found(argument_by_name(&sig, "a"));
        assert_eq!(found.name.as_deref(), Some("a"));
        assert_eq!(found.pos, Some(0));
        assert!(found.required);
    }

    #[test]
    fn argument_by_name_synthesizes_from_kwargs() {
        let callee = callable(vec![(any_type(), ArgKind::Star2, None)]);
        let sig = signature(&callee);
        let found = expect_found(argument_by_name(&sig, "anything"));
        assert_eq!(found.name.as_deref(), Some("anything"));
        assert_eq!(found.pos, None);
        assert!(!found.required);
    }

    #[test]
    fn argument_by_name_absent_without_kwargs() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, Some("a".to_string()))]);
        let sig = signature(&callee);
        assert_eq!(argument_by_name(&sig, "z"), None);
        assert_eq!(synthesize_arg_from_kwarg(&sig, Some("z".to_string())), None);
    }

    #[test]
    fn argument_by_position_reads_an_in_range_formal() {
        let callee = callable(vec![
            (any_type(), ArgKind::Pos, Some("a".to_string())),
            (any_type(), ArgKind::Named, Some("b".to_string())),
        ]);
        let sig = signature(&callee);
        let found = expect_found(argument_by_position(&sig, 0));
        assert_eq!(found.pos, Some(0));
        assert_eq!(argument_by_position(&sig, 1), None);
    }

    #[test]
    fn argument_by_position_synthesizes_from_vararg() {
        let callee = callable(vec![
            (any_type(), ArgKind::Pos, Some("a".to_string())),
            (any_type(), ArgKind::Star, None),
        ]);
        let sig = signature(&callee);
        let found = expect_found(argument_by_position(&sig, 5));
        assert_eq!(found.pos, Some(5));
        assert_eq!(found.name, None);
        assert!(!found.required);
    }

    #[test]
    fn argument_by_position_absent_without_vararg() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, Some("a".to_string()))]);
        let sig = signature(&callee);
        assert_eq!(argument_by_position(&sig, 5), None);
        assert_eq!(synthesize_arg_from_vararg(&sig, Some(5)), None);
    }

    #[test]
    fn callable_corresponding_argument_finds_by_name() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, Some("a".to_string()))]);
        let sig = signature(&callee);
        let model = FormalArgument {
            name: Some("a".to_string()),
            pos: Some(0),
            typ: any_type(),
            required: true,
        };
        match callable_corresponding_argument(&sig, &model) {
            CorrespondingArgument::Found(found) => {
                assert_eq!(found.name.as_deref(), Some("a"));
                assert_eq!(found.pos, Some(0));
            }
            other => panic!("expected a corresponding argument, got {other:?}"),
        }
    }

    #[test]
    fn callable_corresponding_argument_absent_when_nothing_matches() {
        let callee = callable(vec![(any_type(), ArgKind::Pos, Some("a".to_string()))]);
        let sig = signature(&callee);
        let model = FormalArgument {
            name: Some("z".to_string()),
            pos: None,
            typ: any_type(),
            required: true,
        };
        let found = callable_corresponding_argument(&sig, &model);
        assert_eq!(found, CorrespondingArgument::Absent);
    }

    #[test]
    fn map_actuals_to_formals_binds_positionals() {
        let formal = formals(&["x", "y"]);
        let mapped = expect_mapped(map_actuals_to_formals(&positional(2), &formal));
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped.as_slices().to_vec(), vec![vec![0i64], vec![1i64]]);
    }

    #[test]
    fn map_actuals_to_formals_drops_an_overflowing_positional() {
        let formal = formals(&["x"]);
        let mapped = expect_mapped(map_actuals_to_formals(&positional(2), &formal));
        assert_eq!(mapped.as_slices().to_vec(), vec![vec![0i64]]);
        let reverse = expect_reverse(map_formals_to_actuals(&positional(2), &formal));
        assert_eq!(reverse.as_slices().to_vec(), vec![vec![0i64], vec![]]);
    }

    #[test]
    fn map_actuals_to_formals_routes_an_unknown_keyword_to_kwargs() {
        let actual = vec![ActualArg::named("z")];
        let formal = vec![FormalParam::positional("x"), star2_formal()];
        let mapped = expect_mapped(map_actuals_to_formals(&actual, &formal));
        assert_eq!(
            mapped.as_slices().to_vec(),
            vec![Vec::<i64>::new(), vec![0]]
        );
    }

    #[test]
    fn map_actuals_to_formals_defers_on_a_star_actual() {
        let actual = vec![ActualArg::star()];
        let formal = formals(&["x"]);
        assert_eq!(map_actuals_to_formals(&actual, &formal), None);
        assert_eq!(map_formals_to_actuals(&actual, &formal), None);
    }

    #[test]
    fn map_actuals_to_formals_with_types_unpacks_a_tuple_star() {
        let actual = vec![ActualArg::star()];
        let types = StarTypes(vec![Some(tuple_of(vec![any_type(), any_type()]))]);
        let formal = formals(&["x", "y", "z"]);
        let mapped = expect_mapped(map_actuals_to_formals_with_types(&actual, &formal, &types));
        assert_eq!(
            mapped.as_slices().to_vec(),
            vec![vec![0i64], vec![0i64], vec![]]
        );
        let reverse = expect_reverse(map_formals_to_actuals_with_types(&actual, &formal, &types));
        assert_eq!(reverse.as_slices().to_vec(), vec![vec![0i64, 1i64]]);
    }

    #[test]
    fn map_actuals_to_formals_with_types_binds_an_iterable_star() {
        let actual = vec![ActualArg::star()];
        let types = StarTypes(vec![Some(instance("builtins.list"))]);
        let formal = vec![FormalParam::positional("x"), star_formal()];
        let mapped = expect_mapped(map_actuals_to_formals_with_types(&actual, &formal, &types));
        assert_eq!(mapped.as_slices().to_vec(), vec![vec![0i64], vec![0i64]]);
    }

    #[test]
    fn map_actuals_to_formals_with_types_fills_ambiguous_kwargs() {
        let actual = vec![ActualArg::star2()];
        let types = StarTypes(vec![Some(instance("builtins.dict"))]);
        let formal = vec![FormalParam::positional("x"), star2_formal()];
        let mapped = expect_mapped(map_actuals_to_formals_with_types(&actual, &formal, &types));
        assert_eq!(mapped.as_slices().to_vec(), vec![vec![0i64], vec![0i64]]);
    }

    #[test]
    fn map_actuals_to_formals_with_types_defers_without_a_star_type() {
        let actual = vec![ActualArg::star()];
        let types = StarTypes(vec![None]);
        let formal = formals(&["x"]);
        let mapped = map_actuals_to_formals_with_types(&actual, &formal, &types);
        assert_eq!(mapped, None);
    }

    #[test]
    fn real_union_counts_relevant_items() {
        let resolver = TypeResolver::new();
        let union = union_of(vec![instance("builtins.int"), none_type()]);
        assert_eq!(real_union(&union, true, &resolver), Some(true));
        assert_eq!(real_union(&union, false, &resolver), Some(false));
        let plain = instance("builtins.int");
        assert_eq!(real_union(&plain, true, &resolver), Some(false));
    }

    #[test]
    fn possible_none_type_var_overlap_needs_both_a_none_and_a_typevar() {
        let resolver = TypeResolver::new();
        let optional = union_of(vec![none_type(), instance("builtins.int")]);
        let none_target = callable(vec![(none_type(), ArgKind::Pos, None)]);
        let tvar_target = callable(vec![(type_var(), ArgKind::Pos, None)]);
        let targets = vec![none_target, tvar_target];
        let args = vec![optional];
        let overlap = possible_none_type_var_overlap(&args, &targets, &resolver);
        assert_eq!(overlap, Some(true));
        let plain = vec![instance("builtins.int")];
        let overlap = possible_none_type_var_overlap(&plain, &targets, &resolver);
        assert_eq!(overlap, Some(false));
    }

    #[test]
    fn possible_none_type_var_overlap_defers_on_a_non_callable_target() {
        let resolver = TypeResolver::new();
        let optional = union_of(vec![none_type(), instance("builtins.int")]);
        let args = vec![optional];
        let targets = vec![instance("builtins.int")];
        let overlap = possible_none_type_var_overlap(&args, &targets, &resolver);
        assert_eq!(overlap, None);
    }

    #[test]
    fn any_unpack_anywhere_detects_an_unpacked_argument() {
        let unpacking = callable(vec![(unpack(any_type()), ArgKind::Pos, None)]);
        assert!(any_unpack_anywhere(&unpacking));
        let plain = callable(vec![(any_type(), ArgKind::Pos, None)]);
        assert!(!any_unpack_anywhere(&plain));
        assert!(!any_unpack_anywhere(&instance("mod.C")));
    }
}
