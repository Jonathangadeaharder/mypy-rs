//! Standalone-path public API: stmt.
//!
//! Statement checking: assignment, control flow, function and class
//! definitions, return, with, try.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of checker_stmts.rs, checker_functions.rs, checker_driver.rs,
//! checker_visitor.rs, constant_fold.rs rather than reimplementing them;
//! where a helper needs a Python-side callback today, take the callback
//! as a Rust trait object or an explicit record and say so in the doc
//! comment.
//!
//! Owned exclusively by the `stmt` lane for this wave. Add `#[cfg(test)]`
//! unit tests here: they keep the lifted API honest and are the only
//! thing that exercises it before the driver integration wave.
//!
//! # How the hybrid's live-node reads are substituted
//!
//! The hybrid seams these functions wrap read their scalar facts off live
//! Python objects: `rust_classify_check_lvalue` walks the `lvalue` node
//! with `isinstance` probes, `rust_classify_check_assignment` reads
//! `lvalue.node.name` and `lvalue_type`'s `PartialType` shape, and so on.
//! A standalone caller has no Python object to probe, so each operation
//! here takes the same facts as an explicit `*Facts` record that the
//! caller derives from its own AST representation. The record fields are
//! named after the Python attribute each one replaces, and the decision
//! itself is the untouched kernel function: this module lifts logic, it
//! never re-derives it.
//!
//! # `None` means "the kernel declines", not "no problem"
//!
//! Several lifted operations return `Option`. In the hybrid `None` defers
//! to the pure-Python body, which can read the alias target, the live
//! `TypeInfo` or the binder state the wire format does not carry. The
//! standalone path has no such fallback, so a `None` here must be treated
//! by the caller as an unsupported construct that rejects loudly naming
//! the construct (plan rule 6), never as a pass.

pub use crate::typeinfo::{TypeInfoSnapshot, TypeResolver};
pub use crate::wire::Type;

use crate::aliases::TypeAliasResolver;
use crate::checker_functions as functions;
use crate::checker_stmts as stmts;

/// The alias-snapshot store the lifted operations consult when a
/// `TypeAliasType` has to expand (mypy's `get_proper_type`).
///
/// This is an opaque wrapper over the kernel's crate-private
/// `aliases::TypeAliasResolver`: the standalone path needs the type in a
/// public signature, and the resolver itself is not part of the kernel's
/// public surface. Populating it needs a public alias-snapshot record,
/// which is the records lane's product; until that lands the only
/// constructible value is [`AliasContext::empty`], and every operation
/// given an empty context defers (`None`) on an alias exactly as the
/// hybrid does when its snapshot map lacks the entry.
pub struct AliasContext {
    inner: TypeAliasResolver,
}

impl AliasContext {
    /// A context with no alias snapshots: every `TypeAliasType` defers.
    pub fn empty() -> AliasContext {
        AliasContext {
            inner: TypeAliasResolver::new(),
        }
    }
}

impl Default for AliasContext {
    fn default() -> AliasContext {
        AliasContext::empty()
    }
}

// ---------------------------------------------------------------------------
// return checking (mypy/checker.py::TypeChecker.check_return_stmt)
// ---------------------------------------------------------------------------

/// The scalar facts `check_return_stmt` reads off the enclosing function
/// definition and the returned expression.
///
/// Substitution record for the hybrid's live reads: the shim behind
/// `rust_classify_return_stmt_post` computes each of these from
/// `self.scope.stack()` frames, the `FuncDef` flags and the returned
/// expression node. A standalone caller computes them from its own
/// representation of the enclosing definition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReturnStmtFacts {
    /// `self.is_async_generator()`.
    pub is_async_generator: bool,
    /// `self.is_generator()`.
    pub is_generator: bool,
    /// `self.is_coroutine()`.
    pub is_coroutine: bool,
    /// The enclosing def is annotated to return `None`.
    pub declared_none_return: bool,
    /// `self.options.warn_return_any`.
    pub warn_return_any: bool,
    /// `self.current_node_deferred`.
    pub current_node_deferred: bool,
    /// The enclosing def is a binary dunder (`name_in_binary_magic`).
    pub name_in_binary_magic: bool,
    /// The returned expression is the `NotImplemented` literal.
    pub expr_is_literal_not_implemented: bool,
    /// The enclosing def is a lambda.
    pub is_lambda: bool,
    /// `self.in_checked_function()`.
    pub in_checked_function: bool,
}

/// The decision front of `TypeChecker.check_return_stmt`
/// (mypy/checker.py:6576-6627).
///
/// `CheckSubtype` is the only outcome that needs a subtype question: the
/// caller runs `is_subtype(returned, return_type)` and reports
/// INCOMPATIBLE_RETURN_VALUE_TYPE on failure. Every other variant is a
/// terminal accept or a terminal failure with its own error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnOutcome {
    /// checker.py:6576-6580, RETURN_IN_ASYNC_GENERATOR.
    AsyncGeneratorFail,
    /// checker.py:6581-6602 with `warn_return_any`, WARN_RETURN_ANY.
    WarnReturnAny,
    /// checker.py:6581-6602, a returned `Any` is accepted.
    AnyReturnOk,
    /// checker.py:6605-6612, the lambda / `None`-value exemption.
    DeclaredNoneOk,
    /// checker.py:6605-6612, NO_RETURN_VALUE_EXPECTED.
    DeclaredNoneFail,
    /// checker.py:6627, run the `check_subtype` tail.
    CheckSubtype,
    /// checker.py:6613-6626, an empty `return` is accepted.
    EmptyOk,
    /// checker.py:6613-6626, RETURN_VALUE_EXPECTED.
    EmptyFail,
}

/// `TypeChecker.check_return_stmt` (mypy/checker.py:6441-6627), the
/// decision front. `returned` is `None` for a bare `return`, otherwise
/// the already-accepted type of the returned expression; `return_type`
/// is the enclosing definition's proper return type.
///
/// Lifts `checker_functions::classify_return_stmt_post`; the generator /
/// coroutine return-type variant selection (checker.py:6441-6452) stays
/// with the caller because it needs `get_generator_return_type`, which
/// the infer lane exposes.
pub fn check_return_stmt(
    returned: Option<&Type>,
    return_type: &Type,
    facts: &ReturnStmtFacts,
) -> ReturnOutcome {
    let tag = functions::classify_return_stmt_post(
        returned,
        return_type,
        facts.is_async_generator,
        facts.is_generator,
        facts.is_coroutine,
        facts.declared_none_return,
        facts.warn_return_any,
        facts.current_node_deferred,
        facts.name_in_binary_magic,
        facts.expr_is_literal_not_implemented,
        facts.is_lambda,
        facts.in_checked_function,
    );
    match tag {
        functions::RETURN_TAG_ASYNC_GEN_FAIL => ReturnOutcome::AsyncGeneratorFail,
        functions::RETURN_TAG_WARN_ANY => ReturnOutcome::WarnReturnAny,
        functions::RETURN_TAG_ANY_RETURN => ReturnOutcome::AnyReturnOk,
        functions::RETURN_TAG_NONE_OK => ReturnOutcome::DeclaredNoneOk,
        functions::RETURN_TAG_NONE_FAIL => ReturnOutcome::DeclaredNoneFail,
        functions::RETURN_TAG_CHECK_SUBTYPE => ReturnOutcome::CheckSubtype,
        functions::RETURN_TAG_EMPTY_OK => ReturnOutcome::EmptyOk,
        functions::RETURN_TAG_EMPTY_FAIL => ReturnOutcome::EmptyFail,
        other => unreachable!("classify_return_stmt_post produced tag {other}"),
    }
}

/// The NO_RETURN_EXPECTED arm of `TypeChecker.check_return_stmt`
/// (mypy/checker.py:6453-6459): `true` when the enclosing definition's
/// return type is a non-ambiguous `UninhabitedType` and the definition is
/// not a lambda, meaning any `return <value>` must fail.
///
/// Lifts `checker_functions::classify_return_stmt_pre`.
pub fn no_return_expected(return_type: &Type, is_lambda: bool) -> bool {
    functions::classify_return_stmt_pre(return_type, is_lambda)
}

/// The `not is_proper_subtype(AnyType(TypeOfAny.special_form), ret)`
/// clause of the `warn_return_any` gate (mypy/checker.py:6591-6596),
/// decided structurally over the proper return type.
///
/// Lifts `checker_functions::any_is_proper_subtype_of`.
pub fn any_is_proper_subtype_of(return_type: &Type) -> bool {
    functions::any_is_proper_subtype_of(return_type)
}

// ---------------------------------------------------------------------------
// lvalue dispatch (mypy/checker.py::TypeChecker.check_lvalue)
// ---------------------------------------------------------------------------

/// The lvalue-node facts `check_lvalue` dispatches on.
///
/// Substitution record for the `isinstance` probe chain in
/// `rust_classify_check_lvalue`: `is_name` is `NameExpr`, `is_member` is
/// `MemberExpr`, `is_index` is `IndexExpr`, `is_tuple` / `is_list` are
/// `TupleExpr` / `ListExpr`, `is_star` is `StarExpr`, `node_is_var` is
/// `isinstance(lvalue.node, Var)`, and `skip_definition` is the
/// redefinition conjunction that [`lvalue_skip_definition`] computes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LvalueFacts {
    /// `self.is_definition(lvalue)`.
    pub is_definition: bool,
    /// The lvalue is a `NameExpr`.
    pub is_name: bool,
    /// The lvalue is a `MemberExpr`.
    pub is_member: bool,
    /// The lvalue is an `IndexExpr`.
    pub is_index: bool,
    /// The lvalue is a `TupleExpr`.
    pub is_tuple: bool,
    /// The lvalue is a `ListExpr`.
    pub is_list: bool,
    /// The lvalue is a `StarExpr`.
    pub is_star: bool,
    /// `isinstance(lvalue.node, Var)`.
    pub node_is_var: bool,
    /// The `allow_redefinition` conjunction; see [`lvalue_skip_definition`].
    pub skip_definition: bool,
}

/// The `Var`-node facts the `allow_redefinition` conjunction reads.
///
/// Substitution record for the nested attribute reads in
/// `rust_classify_check_lvalue` (mypy/checker.py:5585-5600).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VarNodeFacts {
    /// `isinstance(lvalue.node, Var)`.
    pub is_var: bool,
    /// `lvalue.node.is_inferred`.
    pub is_inferred: bool,
    /// `lvalue.node.type is not None`.
    pub has_type: bool,
    /// `isinstance(lvalue.node.type, PartialType)`.
    pub type_is_partial: bool,
    /// `lvalue.node.is_index_var`.
    pub is_index_var: bool,
}

/// Which arm of `TypeChecker.check_lvalue` (mypy/checker.py:5568-5632)
/// handles the assignment target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvalueKind {
    /// A name being defined: `check_assignment_to_var` with a definition.
    NameDef,
    /// A member being defined: `check_member_assignment` as a definition.
    MemberDef,
    /// `check_indexed_assignment`.
    Index,
    /// `check_member_assignment`.
    Member,
    /// A plain name rebinding: `check_assignment_to_var`.
    Name,
    /// `check_assignment_to_multiple_lvalues`.
    TupleList,
    /// A starred target, handled by the multiple-lvalue arm.
    Star,
    /// No arm matched; mypy falls through to the generic tail.
    Else,
}

/// `TypeChecker.check_lvalue` (mypy/checker.py:5568-5632), the branch
/// dispatch. Lifts `checker_functions::classify_check_lvalue`; the arm
/// bodies (binder writes, message emission) belong to the caller.
pub fn check_lvalue(facts: &LvalueFacts) -> LvalueKind {
    let tag = functions::classify_check_lvalue(
        facts.is_definition,
        facts.is_name,
        facts.is_member,
        facts.is_index,
        facts.is_tuple,
        facts.is_list,
        facts.is_star,
        facts.node_is_var,
        facts.skip_definition,
    );
    match tag {
        functions::KIND_LVALUE_NAME_DEF => LvalueKind::NameDef,
        functions::KIND_LVALUE_MEMBER_DEF => LvalueKind::MemberDef,
        functions::KIND_LVALUE_INDEX => LvalueKind::Index,
        functions::KIND_LVALUE_MEMBER => LvalueKind::Member,
        functions::KIND_LVALUE_NAME => LvalueKind::Name,
        functions::KIND_LVALUE_TUPLE_LIST => LvalueKind::TupleList,
        functions::KIND_LVALUE_STAR => LvalueKind::Star,
        functions::KIND_LVALUE_ELSE => LvalueKind::Else,
        other => unreachable!("classify_check_lvalue produced tag {other}"),
    }
}

/// The `skip_definition` conjunction of `TypeChecker.check_lvalue`
/// (mypy/checker.py:5585-5600): with `--allow-redefinition`, an inferred
/// local that already has a non-partial type and is not a comprehension
/// index variable rebinds instead of re-checking as a definition.
///
/// Ported from the Python conjunction because the hybrid computes it
/// inline inside the pyfunction `rust_classify_check_lvalue`, interleaved
/// with its PyO3 attribute reads, so there is no pure helper to lift.
/// The kernel's pyfunction still holds its own copy; see the lane report
/// for the extraction that would give the two paths one source of truth.
pub fn lvalue_skip_definition(allow_redefinition: bool, var: &VarNodeFacts) -> bool {
    allow_redefinition
        && var.is_var
        && var.is_inferred
        && var.has_type
        && !var.type_is_partial
        && !var.is_index_var
}

// ---------------------------------------------------------------------------
// assignment checking (mypy/checker.py::TypeChecker.check_assignment)
// ---------------------------------------------------------------------------

/// The lvalue-node facts `check_assignment`'s special-name front reads.
///
/// Substitution record for the PyO3 reads in `rust_classify_check_
/// assignment`: `node_name` is `lvalue.node.name` (`None` when the node
/// is falsy or the lvalue is not a name), `lvalue_name` is
/// `lvalue.name` for a `MemberExpr`, and `member_kind_none` is
/// `lvalue.kind is None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssignmentLvalue {
    /// The lvalue is a `NameExpr`.
    pub is_name_expr: bool,
    /// `bool(lvalue.node)` for a `NameExpr`.
    pub node_truthy: bool,
    /// `lvalue.node.name` for a `NameExpr` with a truthy node.
    pub node_name: Option<String>,
    /// The lvalue is a `MemberExpr`.
    pub is_member_expr: bool,
    /// `lvalue.name` for a `MemberExpr`.
    pub lvalue_name: Option<String>,
    /// `lvalue.kind is None` for a `MemberExpr`.
    pub member_kind_none: bool,
}

/// The `lvalue_type` shape facts `check_assignment` dispatches on.
///
/// `None` for the whole record means mypy's `lvalue_type is None`; the
/// hybrid reads `isinstance(lvalue_type, PartialType)` and
/// `lvalue_type.type is None` to fill [`AssignmentLvalueType::partial_none`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssignmentLvalueType {
    /// `isinstance(lvalue_type, PartialType) and lvalue_type.type is None`.
    pub partial_none: bool,
}

/// The special-name front of `TypeChecker.check_assignment`
/// (mypy/checker.py:4692-4720).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentSpecial {
    /// No special-name check applies.
    NoSpecial,
    /// `__setattr__` / `__getattribute__` / `__getattr__`: validate the
    /// signature.
    SetattrSignature,
    /// `__slots__` inside a class body.
    Slots,
    /// `__match_args__` with an inferred `Var`.
    MatchArgs,
    /// `__post_init__`.
    PostInit,
    /// A `MemberExpr` `__match_args__` assignment: always a failure.
    MemberMatchArgs,
}

/// The `lvalue_type` dispatch of `TypeChecker.check_assignment`
/// (mypy/checker.py:4722-4851).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentBranch {
    /// No lvalue type: the indexed arm or the inferred-`Var` tail.
    NoType,
    /// The partial-`None` inference arm.
    PartialNone,
    /// `check_member_assignment` (a member whose `kind is None`).
    Member,
    /// Routes to [`check_simple_assignment`].
    Simple,
}

/// The two decisions `TypeChecker.check_assignment` makes before it runs
/// any arm body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssignmentDecision {
    /// The special-name front result.
    pub special: AssignmentSpecial,
    /// The `lvalue_type` dispatch result.
    pub branch: AssignmentBranch,
}

/// `TypeChecker.check_assignment` (mypy/checker.py:4681-4851), the
/// decision front: the special-name check and the `lvalue_type` dispatch.
/// `has_inferred` is `inferred is not None`, `active_class` is
/// `self.scope.active_class() is not None`.
///
/// Lifts `checker_functions::classify_check_assignment_special` and
/// `classify_check_assignment_branch`; every arm body (signature /
/// slots / match-args checks, partial-`None` inference, binder writes,
/// message emission) belongs to the caller.
pub fn check_assignment(
    lvalue: &AssignmentLvalue,
    lvalue_type: Option<&AssignmentLvalueType>,
    has_inferred: bool,
    active_class: bool,
) -> AssignmentDecision {
    let special = functions::classify_check_assignment_special(
        lvalue.is_name_expr,
        lvalue.node_truthy,
        lvalue.node_name.as_deref(),
        lvalue.lvalue_name.as_deref(),
        lvalue.is_member_expr,
        active_class,
        has_inferred,
    );
    let branch = functions::classify_check_assignment_branch(
        lvalue_type.is_some(),
        lvalue_type.is_some_and(|t| t.partial_none),
        lvalue.is_member_expr,
        lvalue.member_kind_none,
    );
    AssignmentDecision {
        special: match special {
            functions::CA_SPECIAL_NONE => AssignmentSpecial::NoSpecial,
            functions::CA_SPECIAL_SETATTR_SIG => AssignmentSpecial::SetattrSignature,
            functions::CA_SPECIAL_SLOTS => AssignmentSpecial::Slots,
            functions::CA_SPECIAL_MATCH_ARGS => AssignmentSpecial::MatchArgs,
            functions::CA_SPECIAL_POST_INIT => AssignmentSpecial::PostInit,
            functions::CA_SPECIAL_MEMBER_MATCH_ARGS => AssignmentSpecial::MemberMatchArgs,
            other => unreachable!("classify_check_assignment_special produced tag {other}"),
        },
        branch: match branch {
            functions::CA_BRANCH_NO_TYPE => AssignmentBranch::NoType,
            functions::CA_BRANCH_PARTIAL_NONE => AssignmentBranch::PartialNone,
            functions::CA_BRANCH_MEMBER => AssignmentBranch::Member,
            functions::CA_BRANCH_SIMPLE => AssignmentBranch::Simple,
            other => unreachable!("classify_check_assignment_branch produced tag {other}"),
        },
    }
}

/// The facts `check_simple_assignment` reads besides the lvalue type.
///
/// Substitution record for the shim behind `rust_classify_simple_
/// assignment`: `is_stub` is `self.is_stub`, `rvalue_is_ellipsis` is
/// `isinstance(rvalue, EllipsisExpr)`, `has_inferred` /
/// `inferred_is_argument` come from the inferred `Var`, and
/// `simple_rvalue` is `simple_rvalue(rvalue)` already short-circuited to
/// `false` when mypy's try-fallback precondition does not hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimpleAssignmentFacts {
    /// `self.is_stub`.
    pub is_stub: bool,
    /// `isinstance(rvalue, EllipsisExpr)`.
    pub rvalue_is_ellipsis: bool,
    /// `inferred is not None`.
    pub has_inferred: bool,
    /// `inferred.is_argument`.
    pub inferred_is_argument: bool,
    /// `simple_rvalue(rvalue)`, pre-short-circuited by the caller.
    pub simple_rvalue: bool,
}

/// The head dispatch of `TypeChecker.check_simple_assignment`
/// (mypy/checker.py:6325-6436).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleAssignment {
    /// A stub `x: T = ...` initializer: accept without a subtype check.
    StubEllipsis,
    /// The direct arm: no fallback re-inference, `lvalue_type` preferred.
    Direct,
    /// The fallback arm with no preferred type: the inferred context wins.
    FallbackNoPreferred,
    /// The fallback arm with the lvalue type preferred.
    FallbackLvaluePreferred,
}

/// `TypeChecker.check_simple_assignment` (mypy/checker.py:6325-6436),
/// the head dispatch for a single annotated or inferred target.
/// `lvalue_type` is the proper declared / inferred lvalue type, `None`
/// when the caller has none. `None` means the lvalue type is an alias the
/// kernel cannot expand, i.e. a deferral the standalone caller must
/// reject loudly.
///
/// Lifts `checker_functions::classify_simple_assignment`. The
/// need-annotation block and the widening / `check_subtype` tail consume
/// the post-accept rvalue type and stay with the caller.
pub fn check_simple_assignment(
    lvalue_type: Option<&Type>,
    facts: &SimpleAssignmentFacts,
) -> Option<SimpleAssignment> {
    let tag = functions::classify_simple_assignment(
        lvalue_type,
        facts.is_stub,
        facts.rvalue_is_ellipsis,
        facts.has_inferred,
        facts.inferred_is_argument,
        facts.simple_rvalue,
    )?;
    match tag {
        functions::SIMPLE_ASSIGNMENT_STUB => Some(SimpleAssignment::StubEllipsis),
        functions::SIMPLE_ASSIGNMENT_DIRECT => Some(SimpleAssignment::Direct),
        functions::SIMPLE_ASSIGNMENT_FALLBACK_NO_PREFERRED => {
            Some(SimpleAssignment::FallbackNoPreferred)
        }
        functions::SIMPLE_ASSIGNMENT_FALLBACK_LVALUE_PREFERRED => {
            Some(SimpleAssignment::FallbackLvaluePreferred)
        }
        other => unreachable!("classify_simple_assignment produced tag {other}"),
    }
}

// ---------------------------------------------------------------------------
// tuple unpacking (mypy/checker.py::check_rvalue_count_in_assignment)
// ---------------------------------------------------------------------------

/// The target / rvalue arity facts of a multiple assignment.
///
/// Substitution record for the `StarExpr` scan in
/// `rust_classify_rvalue_count`: `star_index` is the index of the first
/// starred target and `rvalue_unpack` is the arity position of the
/// rvalue's unpack item, `None` when the rvalue has no unpack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RvalueCountFacts {
    /// Some target is a `StarExpr`.
    pub has_star: bool,
    /// The index of the first starred target.
    pub star_index: i64,
    /// `len(lvalues)`.
    pub lvalues_len: i64,
    /// The rvalue's item count.
    pub rvalue_count: i64,
    /// The rvalue unpack position, `None` when there is no unpack.
    pub rvalue_unpack: Option<i64>,
}

/// The outcome of `TypeChecker.check_rvalue_count_in_assignment`
/// (mypy/checker.py:5319-5354).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RvalueCount {
    /// The arity matches; proceed to the per-item type checks.
    Pass,
    /// An unpacking rvalue needs a starred target.
    StarRequired,
    /// More targets than an unpacking rvalue can fill.
    UnpackTooManyTargets,
    /// Asymmetric prefix / suffix around the two unpacks: a warning only.
    UnpackAsymmetricWarn,
    /// A starred target with too few rvalue items.
    StarTooFewTargets,
    /// A plain target list whose length differs from the rvalue's.
    WrongCount,
}

/// `TypeChecker.check_rvalue_count_in_assignment`
/// (mypy/checker.py:5319-5354): the arity gate every tuple-unpacking and
/// multiple assignment passes through before its items are typed.
///
/// Lifts `checker_functions::classify_rvalue_count`.
pub fn check_rvalue_count(facts: &RvalueCountFacts) -> RvalueCount {
    let tag = functions::classify_rvalue_count(
        facts.has_star,
        facts.star_index,
        facts.lvalues_len,
        facts.rvalue_count,
        facts.rvalue_unpack,
    );
    match tag {
        functions::RVALUE_COUNT_PASS => RvalueCount::Pass,
        functions::RVALUE_COUNT_FAIL_STAR_REQUIRED => RvalueCount::StarRequired,
        functions::RVALUE_COUNT_FAIL_TOO_MANY => RvalueCount::UnpackTooManyTargets,
        functions::RVALUE_COUNT_WARN_TOO_MANY => RvalueCount::UnpackAsymmetricWarn,
        functions::RVALUE_COUNT_FAIL_WRONG_STAR => RvalueCount::StarTooFewTargets,
        functions::RVALUE_COUNT_FAIL_WRONG => RvalueCount::WrongCount,
        other => unreachable!("classify_rvalue_count produced tag {other}"),
    }
}

/// The facts [`is_valid_inferred_type`] reads about the assignment target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InferredTypeFacts {
    /// The lvalue is a `Final` declaration.
    pub is_lvalue_final: bool,
    /// The lvalue is a class or instance member.
    pub is_lvalue_member: bool,
    /// `self.options.allow_redefinition`.
    pub allow_redefinition: bool,
}

/// `mypy.checker.is_valid_inferred_type` (checker.py:9748-9772) with its
/// `InvalidInferredTypes` visitor (checker.py:9775-9799): whether a type
/// mypy just inferred for a variable may be bound to it at all.
///
/// Lifts `checker_stmts::is_valid_inferred_type_with_aliases`. `None` is
/// a deferral (an alias the context cannot expand); see the module header
/// for what a standalone caller must do with one.
pub fn is_valid_inferred_type(
    typ: &Type,
    facts: &InferredTypeFacts,
    aliases: &AliasContext,
) -> Option<bool> {
    stmts::is_valid_inferred_type_with_aliases(
        typ,
        facts.is_lvalue_final,
        facts.is_lvalue_member,
        facts.allow_redefinition,
        &aliases.inner,
    )
}

// ---------------------------------------------------------------------------
// augmented assignment (mypy/checker.py::infer_operator_assignment_method)
// ---------------------------------------------------------------------------

/// The `"__i" + method[2:]` name computation of
/// `TypeChecker._find_inplace_method` (mypy/checker.py:11514).
///
/// Lifts `checker_functions::inplace_method_name`.
pub fn inplace_method_name(method: &str) -> String {
    functions::inplace_method_name(method)
}

/// `TypeChecker.infer_operator_assignment_method`
/// (mypy/checker.py:11498-11520): the method name an augmented assignment
/// `x <op>= y` resolves to. Returns `(inplace, method)`, where `inplace`
/// is `true` when the operator admits an in-place method and the target's
/// class can read it.
///
/// `in_ops` is `op in operators.ops_with_inplace_method` and `has_inplace`
/// is `typ.type.has_readable_member(inplace_method_name(method))`; the
/// hybrid reads the second off the live `TypeInfo`, so a standalone caller
/// supplies it from its own member table. Lifts
/// `checker_functions::infer_operator_assignment_method_inner`.
pub fn infer_operator_assignment_method(
    in_ops: bool,
    has_inplace: bool,
    method: &str,
) -> (bool, String) {
    functions::infer_operator_assignment_method_inner(in_ops, has_inplace, method)
}

// ---------------------------------------------------------------------------
// raise (mypy/checker.py::TypeChecker.type_check_raise)
// ---------------------------------------------------------------------------

/// The dispatch of `TypeChecker.type_check_raise`
/// (mypy/checker.py:6971-7002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiseOutcome {
    /// The raised expression's type is `DeletedType`: report
    /// `deleted_as_rvalue` and stop.
    Deleted,
    /// An ordinary exception type: run the `BaseException` subtype fence
    /// and, for a `FunctionLike`, the zero-argument `check_call`.
    Plain,
    /// `NotImplemented` was raised: mypy suggests `NotImplementedError`.
    NotImplemented,
}

/// `TypeChecker.type_check_raise` (mypy/checker.py:6971-7002), the
/// dispatch front. `typ` is the proper type of the raised expression and
/// `callee_fullname` is the callee's fullname when the expression is a
/// call of a named reference, `None` otherwise.
///
/// Lifts `checker_functions::classify_type_check_raise`.
pub fn type_check_raise(typ: &Type, callee_fullname: Option<&str>) -> RaiseOutcome {
    match functions::classify_type_check_raise(typ, callee_fullname) {
        functions::RAISE_DELETED => RaiseOutcome::Deleted,
        functions::RAISE_PLAIN => RaiseOutcome::Plain,
        functions::RAISE_NOT_IMPLEMENTED => RaiseOutcome::NotImplemented,
        other => unreachable!("classify_type_check_raise produced tag {other}"),
    }
}

// ---------------------------------------------------------------------------
// with (mypy/checker.py::TypeChecker.visit_with_stmt)
// ---------------------------------------------------------------------------

/// The `__exit__` suppression test of `TypeChecker.visit_with_stmt`
/// (mypy/checker.py:6020-6031): whether the context manager's `__exit__`
/// return type makes the `with` body's exceptions swallowed, which mypy
/// uses to decide the body's reachability.
///
/// Lifts `checker_stmts::with_exit_suppresses_inner`.
pub fn with_exit_suppresses(exit_return_type: &Type, strict_optional: bool) -> bool {
    stmts::with_exit_suppresses_inner(exit_return_type, strict_optional)
}

// ---------------------------------------------------------------------------
// try / except (mypy/checker.py::visit_try_stmt and its helpers)
// ---------------------------------------------------------------------------

/// `TypeChecker.get_types_from_except_handler`
/// (mypy/checker.py:5723-5744): the exception types one `except` clause
/// binds, flattened over tuples, variadic tuples and unions the way
/// `make_simplified_union` flattens them.
///
/// Lifts `checker_stmts::try_handler_union_inner`.
pub fn get_types_from_except_handler(typ: &Type, strict_optional: bool) -> Vec<Type> {
    stmts::try_handler_union_inner(typ, strict_optional)
}

/// The classification of one `except` handler test type.
///
/// Mirrors `checker_stmts::HandlerTestClass`, the per-`ttype` dispatch of
/// `TypeChecker.check_except_handler_test` (mypy/checker.py:5941-5965),
/// re-expressed as a public enum because the kernel's variant is
/// crate-private.
#[derive(Debug, Clone, PartialEq)]
pub enum HandlerTest {
    /// `AnyType`: mypy appends the type as-is and continues.
    Any(Type),
    /// `UninhabitedType`: mypy continues without appending.
    Skip,
    /// Not a valid exception type: mypy fails with INVALID_EXCEPTION_TYPE
    /// and returns `default_exception_type(is_star)` immediately.
    Invalid,
    /// A valid exception type derived from a `FunctionLike` or `TypeType`.
    /// mypy still runs the `is_subtype(..., BaseException)` fence.
    Exception(Type),
}

/// `TypeChecker.check_except_handler_test` (mypy/checker.py:5941-5965),
/// the structural classification of one handler test type.
///
/// Lifts `checker_stmts::classify_except_handler_test_inner`. The
/// `is_star` reclassification fence (checker.py:5971-5978) and the
/// `is_subtype` fence stay with the caller. `None` is a deferral: an
/// alias test type, or a type object whose metaclass the resolver does
/// not carry.
pub fn check_except_handler_test(typ: &Type, resolver: &TypeResolver) -> Option<HandlerTest> {
    match stmts::classify_except_handler_test_inner(typ, resolver)? {
        stmts::HandlerTestClass::Any(t) => Some(HandlerTest::Any(t)),
        stmts::HandlerTestClass::Skip => Some(HandlerTest::Skip),
        stmts::HandlerTestClass::Invalid => Some(HandlerTest::Invalid),
        stmts::HandlerTestClass::Exc(t) => Some(HandlerTest::Exception(t)),
    }
}

// ---------------------------------------------------------------------------
// reachability and unused-awaitable notes
// ---------------------------------------------------------------------------

/// `mypy.checker.is_unreachable_map` (checker.py:8974-8975): whether any
/// narrowed value in a binder frame is uninhabited, which makes the code
/// after it unreachable.
///
/// Lifts `checker_stmts::is_unreachable_map_inner`. `None` is a deferral
/// on an alias value, since expanding it could reveal `Never`.
pub fn is_unreachable_map(types: &[Type]) -> Option<bool> {
    stmts::is_unreachable_map_inner(types)
}

/// The note `mypy.checker.type_requires_usage` (checker.py:5822-5840)
/// selects for a function definition whose return value is discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageRequirement {
    /// `typing.Coroutine`: emit UNUSED_COROUTINE.
    UnusedCoroutine,
    /// The type has a readable `__await__`: emit UNUSED_AWAITABLE.
    UnusedAwaitable,
    /// No note applies.
    NoNote,
}

/// `mypy.checker.type_requires_usage` (checker.py:5822-5840): which
/// unused-awaitable note, if any, a discarded value of this type earns.
///
/// Lifts `checker_stmts::type_requires_usage_inner`. `None` is a
/// deferral: an alias the context cannot expand, or a class whose MRO the
/// resolver does not fully carry, where "absent" and "unknown" cannot be
/// told apart.
pub fn type_requires_usage(
    typ: &Type,
    resolver: &TypeResolver,
    aliases: &AliasContext,
) -> Option<UsageRequirement> {
    match stmts::type_requires_usage_inner(typ, resolver, &aliases.inner)? {
        0 => Some(UsageRequirement::UnusedCoroutine),
        1 => Some(UsageRequirement::UnusedAwaitable),
        2 => Some(UsageRequirement::NoNote),
        other => unreachable!("type_requires_usage_inner produced tag {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::LiteralValue;

    fn instance(type_ref: &str) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args: Vec::new(),
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn union(items: Vec<Type>) -> Type {
        Type::UnionType {
            items,
            uses_pep604_syntax: false,
            can_be_true: true,
            can_be_false: true,
            is_evaluated: false,
            original_str_expr: None,
            original_str_fallback: None,
        }
    }

    fn any_type() -> Type {
        Type::AnyType {
            type_of_any: 0,
            source_any: None,
            missing_import_name: None,
        }
    }

    fn bool_literal(value: bool) -> Type {
        Type::LiteralType {
            fallback: Box::new(instance("builtins.bool")),
            value: LiteralValue::Bool(value),
        }
    }

    // -- check_return_stmt -------------------------------------------------

    #[test]
    fn return_value_against_annotation_needs_the_subtype_tail() {
        let ret = instance("builtins.int");
        let facts = ReturnStmtFacts {
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        let got = instance("builtins.str");
        assert_eq!(
            check_return_stmt(Some(&got), &ret, &facts),
            ReturnOutcome::CheckSubtype
        );
    }

    #[test]
    fn return_value_in_a_none_function_is_rejected() {
        let facts = ReturnStmtFacts {
            declared_none_return: true,
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        let got = instance("builtins.int");
        assert_eq!(
            check_return_stmt(Some(&got), &Type::NoneType, &facts),
            ReturnOutcome::DeclaredNoneFail
        );
    }

    #[test]
    fn return_none_in_a_none_function_is_accepted() {
        let facts = ReturnStmtFacts {
            declared_none_return: true,
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        assert_eq!(
            check_return_stmt(Some(&Type::NoneType), &Type::NoneType, &facts),
            ReturnOutcome::DeclaredNoneOk
        );
    }

    #[test]
    fn empty_return_in_a_checked_value_function_is_rejected() {
        let facts = ReturnStmtFacts {
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        let ret = instance("builtins.int");
        assert_eq!(
            check_return_stmt(None, &ret, &facts),
            ReturnOutcome::EmptyFail
        );
    }

    #[test]
    fn empty_return_in_an_unchecked_function_is_accepted() {
        let facts = ReturnStmtFacts::default();
        let ret = instance("builtins.int");
        assert_eq!(
            check_return_stmt(None, &ret, &facts),
            ReturnOutcome::EmptyOk
        );
    }

    #[test]
    fn returned_any_warns_only_under_warn_return_any() {
        let facts = ReturnStmtFacts {
            warn_return_any: true,
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        let ret = instance("builtins.int");
        assert_eq!(
            check_return_stmt(Some(&any_type()), &ret, &facts),
            ReturnOutcome::WarnReturnAny
        );
        let quiet = ReturnStmtFacts {
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        assert_eq!(
            check_return_stmt(Some(&any_type()), &ret, &quiet),
            ReturnOutcome::AnyReturnOk
        );
    }

    #[test]
    fn async_generator_return_is_rejected() {
        let facts = ReturnStmtFacts {
            is_async_generator: true,
            in_checked_function: true,
            ..ReturnStmtFacts::default()
        };
        let ret = instance("builtins.int");
        let got = instance("builtins.int");
        assert_eq!(
            check_return_stmt(Some(&got), &ret, &facts),
            ReturnOutcome::AsyncGeneratorFail
        );
    }

    #[test]
    fn uninhabited_return_type_forbids_a_value() {
        let never = Type::UninhabitedType { ambiguous: false };
        assert!(no_return_expected(&never, false));
        assert!(!no_return_expected(&never, true));
        let ambiguous = Type::UninhabitedType { ambiguous: true };
        assert!(!no_return_expected(&ambiguous, false));
        assert!(!no_return_expected(&instance("builtins.int"), false));
    }

    #[test]
    fn proper_subtype_of_any_holds_for_any_and_unions_of_any() {
        assert!(any_is_proper_subtype_of(&any_type()));
        let mixed = union(vec![instance("builtins.int"), any_type()]);
        assert!(any_is_proper_subtype_of(&mixed));
        assert!(!any_is_proper_subtype_of(&instance("builtins.int")));
    }

    // -- check_lvalue ------------------------------------------------------

    #[test]
    fn a_new_name_binds_as_a_definition() {
        let facts = LvalueFacts {
            is_definition: true,
            is_name: true,
            node_is_var: true,
            ..LvalueFacts::default()
        };
        assert_eq!(check_lvalue(&facts), LvalueKind::NameDef);
    }

    #[test]
    fn an_existing_name_rebinds_without_a_definition() {
        let facts = LvalueFacts {
            is_name: true,
            node_is_var: true,
            ..LvalueFacts::default()
        };
        assert_eq!(check_lvalue(&facts), LvalueKind::Name);
    }

    #[test]
    fn a_starred_target_routes_to_the_multiple_lvalue_arm() {
        let facts = LvalueFacts {
            is_star: true,
            ..LvalueFacts::default()
        };
        assert_eq!(check_lvalue(&facts), LvalueKind::Star);
    }

    #[test]
    fn redefinition_skips_the_definition_arm() {
        let base = LvalueFacts {
            is_definition: true,
            is_name: true,
            node_is_var: true,
            skip_definition: true,
            ..LvalueFacts::default()
        };
        assert_eq!(check_lvalue(&base), LvalueKind::Name);
    }

    #[test]
    fn skip_definition_needs_every_conjunct() {
        let full = VarNodeFacts {
            is_var: true,
            is_inferred: true,
            has_type: true,
            type_is_partial: false,
            is_index_var: false,
        };
        assert!(lvalue_skip_definition(true, &full));
        assert!(!lvalue_skip_definition(false, &full));
        assert!(!lvalue_skip_definition(
            true,
            &VarNodeFacts {
                type_is_partial: true,
                ..full
            }
        ));
        assert!(!lvalue_skip_definition(
            true,
            &VarNodeFacts {
                is_index_var: true,
                ..full
            }
        ));
        assert!(!lvalue_skip_definition(
            true,
            &VarNodeFacts {
                is_inferred: false,
                ..full
            }
        ));
    }

    // -- check_assignment --------------------------------------------------

    fn name_lvalue(name: &str) -> AssignmentLvalue {
        AssignmentLvalue {
            is_name_expr: true,
            node_truthy: true,
            node_name: Some(name.to_string()),
            ..AssignmentLvalue::default()
        }
    }

    #[test]
    fn an_ordinary_name_assignment_is_a_simple_definition() {
        let lv = name_lvalue("x");
        let ty = AssignmentLvalueType::default();
        let got = check_assignment(&lv, Some(&ty), true, false);
        assert_eq!(got.special, AssignmentSpecial::NoSpecial);
        assert_eq!(got.branch, AssignmentBranch::Simple);
    }

    #[test]
    fn slots_is_special_only_inside_a_class_body() {
        let lv = name_lvalue("__slots__");
        assert_eq!(
            check_assignment(&lv, None, false, true).special,
            AssignmentSpecial::Slots
        );
        assert_eq!(
            check_assignment(&lv, None, false, false).special,
            AssignmentSpecial::NoSpecial
        );
    }

    #[test]
    fn a_member_match_args_assignment_always_fails() {
        let lv = AssignmentLvalue {
            is_member_expr: true,
            lvalue_name: Some("__match_args__".to_string()),
            ..AssignmentLvalue::default()
        };
        assert_eq!(
            check_assignment(&lv, None, false, true).special,
            AssignmentSpecial::MemberMatchArgs
        );
    }

    #[test]
    fn a_missing_lvalue_type_takes_the_no_type_branch() {
        let lv = name_lvalue("x");
        let got = check_assignment(&lv, None, false, false);
        assert_eq!(got.branch, AssignmentBranch::NoType);
    }

    #[test]
    fn a_partial_none_lvalue_type_takes_the_inference_branch() {
        let lv = name_lvalue("x");
        let ty = AssignmentLvalueType { partial_none: true };
        let got = check_assignment(&lv, Some(&ty), false, false);
        assert_eq!(got.branch, AssignmentBranch::PartialNone);
    }

    #[test]
    fn a_kindless_member_routes_to_the_member_branch() {
        let lv = AssignmentLvalue {
            is_member_expr: true,
            member_kind_none: true,
            ..AssignmentLvalue::default()
        };
        let ty = AssignmentLvalueType::default();
        let got = check_assignment(&lv, Some(&ty), false, false);
        assert_eq!(got.branch, AssignmentBranch::Member);
    }

    // -- check_simple_assignment -------------------------------------------

    #[test]
    fn a_declared_target_checks_directly() {
        let facts = SimpleAssignmentFacts::default();
        let declared = instance("builtins.int");
        assert_eq!(
            check_simple_assignment(Some(&declared), &facts),
            Some(SimpleAssignment::Direct)
        );
    }

    #[test]
    fn a_stub_ellipsis_initializer_is_accepted_unchecked() {
        let facts = SimpleAssignmentFacts {
            is_stub: true,
            rvalue_is_ellipsis: true,
            ..SimpleAssignmentFacts::default()
        };
        let declared = instance("builtins.int");
        assert_eq!(
            check_simple_assignment(Some(&declared), &facts),
            Some(SimpleAssignment::StubEllipsis)
        );
    }

    #[test]
    fn a_union_target_with_an_inferred_non_argument_falls_back() {
        let facts = SimpleAssignmentFacts {
            has_inferred: true,
            inferred_is_argument: false,
            ..SimpleAssignmentFacts::default()
        };
        let target = union(vec![instance("builtins.int"), instance("builtins.str")]);
        assert_eq!(
            check_simple_assignment(Some(&target), &facts),
            Some(SimpleAssignment::FallbackNoPreferred)
        );
    }

    #[test]
    fn an_inferred_argument_target_prefers_the_lvalue_type() {
        let facts = SimpleAssignmentFacts {
            has_inferred: true,
            inferred_is_argument: true,
            ..SimpleAssignmentFacts::default()
        };
        let target = union(vec![instance("builtins.int"), instance("builtins.str")]);
        assert_eq!(
            check_simple_assignment(Some(&target), &facts),
            Some(SimpleAssignment::FallbackLvaluePreferred)
        );
    }

    #[test]
    fn a_simple_rvalue_never_enters_the_fallback_arm() {
        let facts = SimpleAssignmentFacts {
            has_inferred: true,
            simple_rvalue: true,
            ..SimpleAssignmentFacts::default()
        };
        let target = union(vec![instance("builtins.int"), instance("builtins.str")]);
        assert_eq!(
            check_simple_assignment(Some(&target), &facts),
            Some(SimpleAssignment::Direct)
        );
    }

    // -- check_rvalue_count ------------------------------------------------

    #[test]
    fn matching_arity_passes() {
        let facts = RvalueCountFacts {
            lvalues_len: 2,
            rvalue_count: 2,
            ..RvalueCountFacts::default()
        };
        assert_eq!(check_rvalue_count(&facts), RvalueCount::Pass);
    }

    #[test]
    fn a_plain_count_mismatch_is_rejected() {
        let facts = RvalueCountFacts {
            lvalues_len: 3,
            rvalue_count: 2,
            ..RvalueCountFacts::default()
        };
        assert_eq!(check_rvalue_count(&facts), RvalueCount::WrongCount);
    }

    #[test]
    fn a_starred_target_absorbs_extra_items() {
        let facts = RvalueCountFacts {
            has_star: true,
            star_index: 1,
            lvalues_len: 2,
            rvalue_count: 5,
            ..RvalueCountFacts::default()
        };
        assert_eq!(check_rvalue_count(&facts), RvalueCount::Pass);
    }

    #[test]
    fn a_starred_target_still_needs_one_item_each() {
        let facts = RvalueCountFacts {
            has_star: true,
            star_index: 1,
            lvalues_len: 3,
            rvalue_count: 1,
            ..RvalueCountFacts::default()
        };
        assert_eq!(check_rvalue_count(&facts), RvalueCount::StarTooFewTargets);
    }

    #[test]
    fn an_unpacking_rvalue_requires_a_starred_target() {
        let facts = RvalueCountFacts {
            lvalues_len: 2,
            rvalue_count: 2,
            rvalue_unpack: Some(1),
            ..RvalueCountFacts::default()
        };
        assert_eq!(check_rvalue_count(&facts), RvalueCount::StarRequired);
    }

    #[test]
    fn asymmetric_unpack_prefixes_warn() {
        let facts = RvalueCountFacts {
            has_star: true,
            star_index: 2,
            lvalues_len: 3,
            rvalue_count: 3,
            rvalue_unpack: Some(0),
            ..RvalueCountFacts::default()
        };
        assert_eq!(
            check_rvalue_count(&facts),
            RvalueCount::UnpackAsymmetricWarn
        );
    }

    // -- is_valid_inferred_type --------------------------------------------

    #[test]
    fn an_inferred_none_needs_final_or_redefinition() {
        let facts = InferredTypeFacts::default();
        let aliases = AliasContext::empty();
        assert_eq!(
            is_valid_inferred_type(&Type::NoneType, &facts, &aliases),
            Some(false)
        );
        let final_facts = InferredTypeFacts {
            is_lvalue_final: true,
            ..InferredTypeFacts::default()
        };
        assert_eq!(
            is_valid_inferred_type(&Type::NoneType, &final_facts, &aliases),
            Some(true)
        );
    }

    #[test]
    fn an_inhabited_instance_is_a_valid_inferred_type() {
        let facts = InferredTypeFacts::default();
        let aliases = AliasContext::empty();
        let got = instance("builtins.int");
        assert_eq!(is_valid_inferred_type(&got, &facts, &aliases), Some(true));
    }

    #[test]
    fn an_uninhabited_inferred_type_is_never_valid() {
        let facts = InferredTypeFacts::default();
        let aliases = AliasContext::empty();
        let never = Type::UninhabitedType { ambiguous: false };
        assert_eq!(
            is_valid_inferred_type(&never, &facts, &aliases),
            Some(false)
        );
    }

    #[test]
    fn an_alias_defers_without_a_snapshot() {
        let facts = InferredTypeFacts::default();
        let aliases = AliasContext::empty();
        let alias = Type::TypeAliasType {
            args: Vec::new(),
            type_ref: "mod.A".to_string(),
            is_recursive: false,
        };
        assert_eq!(is_valid_inferred_type(&alias, &facts, &aliases), None);
    }

    // -- augmented assignment ----------------------------------------------

    #[test]
    fn an_inplace_capable_target_uses_the_inplace_method() {
        assert_eq!(
            infer_operator_assignment_method(true, true, "__add__"),
            (true, "__iadd__".to_string())
        );
    }

    #[test]
    fn a_target_without_the_inplace_member_falls_back() {
        assert_eq!(
            infer_operator_assignment_method(true, false, "__add__"),
            (false, "__add__".to_string())
        );
        assert_eq!(
            infer_operator_assignment_method(false, false, "__add__"),
            (false, "__add__".to_string())
        );
    }

    #[test]
    fn the_inplace_name_keeps_every_character_after_the_operator() {
        assert_eq!(inplace_method_name("__truediv__"), "__itruediv__");
        assert_eq!(inplace_method_name("__floordiv__"), "__ifloordiv__");
    }

    // -- type_check_raise ---------------------------------------------------

    #[test]
    fn an_exception_instance_raises_plainly() {
        let exc = instance("builtins.ValueError");
        assert_eq!(type_check_raise(&exc, None), RaiseOutcome::Plain);
    }

    #[test]
    fn a_deleted_type_short_circuits_every_other_arm() {
        let deleted = Type::DeletedType { source: None };
        assert_eq!(type_check_raise(&deleted, None), RaiseOutcome::Deleted);
        assert_eq!(
            type_check_raise(&deleted, Some("builtins.NotImplemented")),
            RaiseOutcome::Deleted
        );
    }

    #[test]
    fn raising_not_implemented_is_called_out() {
        let notimpl = instance("builtins._NotImplementedType");
        assert_eq!(
            type_check_raise(&notimpl, None),
            RaiseOutcome::NotImplemented
        );
        let exc = instance("builtins.ValueError");
        assert_eq!(
            type_check_raise(&exc, Some("builtins.NotImplemented")),
            RaiseOutcome::NotImplemented
        );
    }

    // -- with ---------------------------------------------------------------

    #[test]
    fn a_true_literal_exit_suppresses() {
        assert!(with_exit_suppresses(&bool_literal(true), true));
        assert!(with_exit_suppresses(&bool_literal(true), false));
        assert!(!with_exit_suppresses(&bool_literal(false), true));
    }

    #[test]
    fn a_plain_bool_exit_suppresses_only_under_strict_optional() {
        let plain = instance("builtins.bool");
        assert!(with_exit_suppresses(&plain, true));
        assert!(!with_exit_suppresses(&plain, false));
        assert!(!with_exit_suppresses(&instance("builtins.str"), true));
    }

    // -- try / except --------------------------------------------------------

    #[test]
    fn a_single_exception_type_binds_itself() {
        let exc = instance("builtins.ValueError");
        assert_eq!(get_types_from_except_handler(&exc, true), vec![exc]);
    }

    #[test]
    fn a_tuple_handler_binds_every_item() {
        let tup = Type::TupleType {
            partial_fallback: Box::new(instance("builtins.tuple")),
            items: vec![
                instance("builtins.ValueError"),
                instance("builtins.KeyError"),
            ],
            implicit: false,
        };
        assert_eq!(
            get_types_from_except_handler(&tup, true),
            vec![
                instance("builtins.ValueError"),
                instance("builtins.KeyError")
            ]
        );
    }

    #[test]
    fn a_non_strict_optional_union_drops_none() {
        let u = union(vec![instance("builtins.ValueError"), Type::NoneType]);
        assert_eq!(
            get_types_from_except_handler(&u, false),
            vec![instance("builtins.ValueError")]
        );
        assert_eq!(get_types_from_except_handler(&u, true).len(), 2);
    }

    #[test]
    fn a_non_exception_handler_test_is_invalid() {
        let resolver = TypeResolver::new();
        let got = instance("builtins.int");
        assert_eq!(
            check_except_handler_test(&got, &resolver),
            Some(HandlerTest::Invalid)
        );
    }

    #[test]
    fn an_any_handler_test_is_passed_through() {
        let resolver = TypeResolver::new();
        assert_eq!(
            check_except_handler_test(&any_type(), &resolver),
            Some(HandlerTest::Any(any_type()))
        );
    }

    #[test]
    fn an_uninhabited_handler_test_is_skipped() {
        let resolver = TypeResolver::new();
        let never = Type::UninhabitedType { ambiguous: false };
        assert_eq!(
            check_except_handler_test(&never, &resolver),
            Some(HandlerTest::Skip)
        );
    }

    #[test]
    fn a_type_object_handler_yields_its_instance_type() {
        let exc = instance("builtins.ValueError");
        let type_obj = Type::TypeType {
            item: Box::new(exc.clone()),
            is_type_form: false,
        };
        let resolver = TypeResolver::new();
        assert_eq!(
            check_except_handler_test(&type_obj, &resolver),
            Some(HandlerTest::Exception(exc))
        );
    }

    // -- reachability and usage notes -----------------------------------------

    #[test]
    fn an_uninhabited_binder_value_makes_code_unreachable() {
        let never = Type::UninhabitedType { ambiguous: false };
        let values = [instance("builtins.int"), never];
        assert_eq!(is_unreachable_map(&values), Some(true));
    }

    #[test]
    fn inhabited_binder_values_stay_reachable() {
        let values = [instance("builtins.int"), instance("builtins.str")];
        assert_eq!(is_unreachable_map(&values), Some(false));
        assert_eq!(is_unreachable_map(&[]), Some(false));
    }

    #[test]
    fn an_alias_binder_value_defers_the_reachability_question() {
        let alias = Type::TypeAliasType {
            args: Vec::new(),
            type_ref: "mod.A".to_string(),
            is_recursive: false,
        };
        assert_eq!(is_unreachable_map(&[alias]), None);
    }

    #[test]
    fn a_coroutine_return_earns_the_unused_coroutine_note() {
        let resolver = TypeResolver::new();
        let aliases = AliasContext::empty();
        let coro = instance("typing.Coroutine");
        assert_eq!(
            type_requires_usage(&coro, &resolver, &aliases),
            Some(UsageRequirement::UnusedCoroutine)
        );
    }

    #[test]
    fn an_awaitable_return_earns_the_unused_awaitable_note() {
        let mut snap = TypeInfoSnapshot {
            fullname: "mod.Await".to_string(),
            mro: vec!["mod.Await".to_string()],
            ..TypeInfoSnapshot::default()
        };
        snap.member_info
            .insert("__await__".to_string(), (false, true));
        let mut resolver = TypeResolver::new();
        resolver.insert("mod.Await".to_string(), snap);
        let aliases = AliasContext::empty();
        let got = instance("mod.Await");
        assert_eq!(
            type_requires_usage(&got, &resolver, &aliases),
            Some(UsageRequirement::UnusedAwaitable)
        );
    }

    #[test]
    fn a_plain_class_needs_no_usage_note() {
        let snap = TypeInfoSnapshot {
            fullname: "mod.Plain".to_string(),
            mro: vec!["mod.Plain".to_string()],
            ..TypeInfoSnapshot::default()
        };
        let mut resolver = TypeResolver::new();
        resolver.insert("mod.Plain".to_string(), snap);
        let aliases = AliasContext::empty();
        let got = instance("mod.Plain");
        assert_eq!(
            type_requires_usage(&got, &resolver, &aliases),
            Some(UsageRequirement::NoNote)
        );
    }

    #[test]
    fn an_incomplete_mro_defers_the_usage_question() {
        let resolver = TypeResolver::new();
        let aliases = AliasContext::empty();
        let got = instance("builtins.int");
        assert_eq!(type_requires_usage(&got, &resolver, &aliases), None);
    }
}
