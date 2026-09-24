//! Standalone-path public API: expr.
//!
//! Expression checking: operators, comprehensions and generators,
//! f-strings/format, literal and container expressions.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of checkexpr_functions.rs, checkoperator.rs, checkstrformat.rs,
//! generators.rs, subexpr_strip.rs rather than reimplementing them; where
//! a helper needs a Python-side callback today, take the callback as a
//! Rust trait object or an explicit record and say so in the doc comment.
//!
//! Owned exclusively by the `expr` lane for this wave. Add `#[cfg(test)]`
//! unit tests here: they keep the lifted API honest and are the only
//! thing that exercises it before the driver integration wave.
//!
//! # `None` is an answer the driver must reject on
//!
//! Every function here returns `Option<_>`, and `None` always means the
//! kernel declined to decide: an operand shape it cannot expand (a
//! `TypeAliasType`, whose target the wire format does not carry), a
//! subtype question the nominal engine cannot judge, or a snapshot missing
//! from the resolver. The hybrid hands `None` back to the Python body that
//! owns the same decision. The standalone path has no Python body, so
//! `None` must surface as a loud out-of-subset rejection naming the
//! construct (plan rule 6). Nothing here substitutes a default for `None`,
//! because a defaulted answer is a wrong answer that looks like a right
//! one.
//!
//! The types these signatures name (`wire::Type`, `typeinfo::TypeResolver`)
//! reach a standalone caller through `crate::skeleton_api`, the surface the
//! skeleton crate already consumes. This module deliberately adds no second
//! re-export path for them.
//!
//! # Inventory
//!
//! Operator typing (`mypy/checkexpr.py`, `mypy/operators.py`):
//!
//! - [`check_op_reversible_variant_order`] is
//!   `ExpressionChecker.check_op_reversible` STEP 2a (checkexpr.py:4695),
//!   lifted from `checkoperator::operator_plan_inner`.
//! - [`lookup_operator_definer`] is `ExpressionChecker.lookup_definer`
//!   (checkexpr.py:5874), lifted from `checkoperator::lookup_definer`.
//! - [`op_method_shortcuts`] is membership in
//!   `operators.op_methods_that_shortcut` (operators.py:150).
//! - [`op_falls_back_to_cmp`] is membership in
//!   `operators.ops_falling_back_to_cmp` (operators.py:147).
//! - [`reverse_op_method`] is a `operators.reverse_op_methods` lookup
//!   (operators.py:55).
//!
//! Literal and container expressions (`mypy/checkexpr.py`,
//! `mypy/checker.py`):
//!
//! - [`try_getting_literal`] is `checkexpr.try_getting_literal`
//!   (checkexpr.py:9348).
//! - [`try_getting_int_literals`] is the type-level half of
//!   `ExpressionChecker.try_getting_int_literals` (checkexpr.py:6710).
//! - [`is_string_literal`] is `checker.is_string_literal`
//!   (checker.py:12504).
//! - [`tuple_context_matches`] is
//!   `ExpressionChecker.tuple_context_matches` (checkexpr.py:7303).
//!
//! Generator and comprehension element typing (`mypy/checker.py`). A
//! comprehension or generator expression is typed by extracting the yield,
//! receive and return parameters of the return type its `return` statements
//! produce, which is exactly what these six decide:
//!
//! - [`is_generator_return_type`] (checker.py:1422)
//! - [`is_async_generator_return_type`] (checker.py:1441)
//! - [`get_generator_yield_type`] (checker.py:1454)
//! - [`get_generator_receive_type`] (checker.py:1488)
//! - [`get_coroutine_return_type`] (checker.py:1523)
//! - [`get_generator_return_type`] (checker.py:1531)
//!
//! Format strings, `%`-interpolation and `str.format()` / f-strings
//! (`mypy/checkstrformat.py`). These are the parsing and classification
//! halves; the argument-type checking half (`StringFormatterChecker`) needs
//! a checker and stays with the driver:
//!
//! - [`parse_conversion_specifiers`] (checkstrformat.py:309)
//! - [`find_non_escaped_targets`] (checkstrformat.py:415)
//! - [`parse_format_value`] (checkstrformat.py:323)
//! - [`parse_placeholder_format`] (checkstrformat.py:178)
//! - [`analyze_conversion_specifiers`] (checkstrformat.py:896)
//! - [`is_numeric_format_type`] (checkstrformat.py:170)
//!
//! The kernel returns these as positional tuples and error integers, which
//! is what a Python caller wants and what a Rust caller cannot read. Here
//! they become named structs, and the five error integers become
//! [`FormatStringError`], one variant per `msg.fail` call in mypy. The
//! message text itself is deliberately not here: `standalone::diag` owns
//! rendering, so this module only says which of the five mypy reports.
//!
//! # Recorded gaps, so the next increment does not rediscover them
//!
//! Everything in the kernel's expression modules that is not exposed above
//! is blocked, and each blocker is one of four kinds.
//!
//! Blocked on `crate::aliases::TypeAliasResolver` visibility. Ten pure-Rust
//! `checkexpr_functions` entries take `&TypeAliasResolver`, which is
//! `pub(crate)` and so cannot be named in a public signature:
//! `has_any_type_inner`, `has_uninhabited_component_inner`,
//! `has_ambiguous_uninhabited_component_inner`, `has_erased_component_inner`,
//! `has_bytes_component_inner`, `is_duplicate_mapping_inner`,
//! `is_type_type_context_inner`, `is_typeddict_type_context_inner`,
//! `allow_fast_container_literal_inner` and `combined_context_inner`. One
//! edit unblocks all ten: make `TypeAliasResolver` and `TypeAliasSnapshot`
//! public, or re-export them from `skeleton_api` next to `TypeResolver`.
//! Alias snapshots are semantic facts, so the `records` lane is the natural
//! owner. Most of the ten are type queries rather than expression checks,
//! which is a second reason the fix belongs in that shared layer and not in
//! any one area module.
//!
//! Blocked on a wire-byte return. `conditional_join_inner` (the port of
//! `join.join_types`, the conditional-expression join),
//! `visit_tuple_index_helper_inner` and `visit_tuple_slice_helper_inner`
//! (tuple subscript and slice expressions) all return `Option<Vec<u8>>`, so
//! reaching them would put a wire decode in the standalone call path, which
//! plan rule 4 forbids. Returning `Option<Type>` and moving the
//! `encode_type` into each `#[pyfunction]` wrapper is behaviour-preserving
//! for the hybrid, but that is a signature change rather than a visibility
//! change. The general join is the `types` lane's deliverable, so exposing
//! it there and composing here is probably right for the first of the three.
//!
//! Blocked on pyo3 in the signature. `classify_check_boolean_op` takes
//! `Python<'_>` and `&NativeTypeResolver`, so it is not Python-free and is
//! not liftable as written.
//!
//! Owned elsewhere. `compute_arg_context_indices_inner` (the index core of
//! `infer_arg_types_in_context`), `is_valid_var_arg_inner` and
//! `is_valid_keyword_var_arg_inner` are pure Rust and liftable, but they are
//! actual-to-formal argument mapping, which is the `call` lane's first
//! deliverable. `is_typed_callable_inner` is pure and liftable but is
//! callable and decorator typing rather than expression typing. Left alone
//! so each operation keeps exactly one owner.
//!
//! Nothing to lift. `subexpr_strip.rs` is entirely a live-`PyAny` walk with
//! no wire-format twin. The standalone driver gets subexpression
//! enumeration from its own lowered AST instead.

use crate::checkexpr_functions::is_string_literal_inner;
use crate::checkexpr_functions::try_getting_int_literals_inner;
use crate::checkexpr_functions::try_getting_literal_inner;
use crate::checkexpr_functions::tuple_context_matches_inner;
use crate::checkoperator::is_shortcut_op;
use crate::checkoperator::lookup_definer;
use crate::checkoperator::operator_plan_inner;
use crate::checkoperator::OP_VARIANT_NORMAL;
use crate::checkoperator::OP_VARIANT_REVERSE_FIRST;
use crate::checkoperator::OP_VARIANT_SHORTCUT_SINGLE;
use crate::checkstrformat::rust_analyze_conversion_specifiers;
use crate::checkstrformat::rust_find_non_escaped_targets;
use crate::checkstrformat::rust_is_numeric_format_type;
use crate::checkstrformat::rust_parse_conversion_specifiers;
use crate::checkstrformat::rust_parse_format_value;
use crate::checkstrformat::rust_parse_placeholder_format;
use crate::checkstrformat::ERR_INVALID_SPECIFIER;
use crate::checkstrformat::ERR_KEY_HAS_BRACE;
use crate::checkstrformat::ERR_NESTING_TOO_DEEP;
use crate::checkstrformat::ERR_UNEXPECTED_CLOSE;
use crate::checkstrformat::ERR_UNMATCHED_OPEN;
use crate::generators::get_coroutine_return_type_inner;
use crate::generators::get_generator_receive_type_inner;
use crate::generators::get_generator_return_type_inner;
use crate::generators::get_generator_yield_type_inner;
use crate::generators::is_async_generator_return_type as async_generator_return_type_inner;
use crate::generators::is_generator_return_type as generator_return_type_inner;
use crate::operators::falls_back_to_cmp;
use crate::operators::get_reverse_op_method;
use crate::typeinfo::TypeResolver;
use crate::wire::Type;

/// The order in which Python calls a binary operator's dunder methods.
///
/// Lifts the three `variants_raw` construction branches of
/// `ExpressionChecker.check_op_reversible` (mypy/checkexpr.py:4702), which
/// `checkoperator::operator_plan_inner` returns as an integer code. The
/// enum replaces that code so no caller has to know the encoding: there is
/// no fourth variant, and a code the kernel does not define becomes `None`
/// rather than a silent default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorVariantOrder {
    /// `op_name` is in `operators.op_methods_that_shortcut` and both
    /// operands are the same type, so only `__op__` is called
    /// (checkexpr.py:4695).
    ShortcutSingle,
    /// The right operand's class covers the left at runtime, so the
    /// reflected `__rop__` is tried before `__op__` (checkexpr.py:4706).
    ReverseFirst,
    /// `__op__` is tried first, then `__rop__` (checkexpr.py:4726).
    Normal,
}

/// Where a binary operator's dunder is defined along a class's MRO.
///
/// Replaces the `Option<Option<String>>` that
/// `checkoperator::lookup_definer` returns, whose outer `None` means
/// "defer" and whose inner `None` means "no class defines it". Those two
/// are different facts with different consequences, and collapsing them
/// would turn mypy's call-variant filtering into a wrong answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorDefiner {
    /// The fullname of the first MRO class that defines the dunder.
    Found(String),
    /// No class in the MRO defines it, so mypy drops that call variant
    /// instead of trying it.
    Absent,
}

/// The dunder calling order for one binary operator application.
///
/// Lifts `checkoperator::operator_plan_inner`, the port of
/// `ExpressionChecker.check_op_reversible` STEP 2a
/// (mypy/checkexpr.py:4695). `op_name` is the non-reversed dunder
/// (`__add__`, never `__radd__`); the reflected name is derived internally
/// through [`reverse_op_method`].
///
/// This decides *order only*. Trying each variant against the operand
/// types and rendering `Unsupported operand types for ...` is the caller's
/// work, as it is in mypy.
///
/// `None` means the kernel declined: an `Any` or `TypeAliasType` operand,
/// a subtype or covers question the Rust engine cannot judge, or a
/// snapshot missing from `resolver`.
pub fn check_op_reversible_variant_order(
    op_name: &str,
    left: &Type,
    right: &Type,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<OperatorVariantOrder> {
    let code = operator_plan_inner(op_name, left, right, resolver, strict_optional)?;
    match code {
        OP_VARIANT_SHORTCUT_SINGLE => Some(OperatorVariantOrder::ShortcutSingle),
        OP_VARIANT_REVERSE_FIRST => Some(OperatorVariantOrder::ReverseFirst),
        OP_VARIANT_NORMAL => Some(OperatorVariantOrder::Normal),
        _ => None,
    }
}

/// The class that defines one dunder along a type's MRO.
///
/// Lifts `checkoperator::lookup_definer`, the port of
/// `ExpressionChecker.lookup_definer` (mypy/checkexpr.py:5874).
///
/// [`OperatorDefiner::Absent`] is a decision, not a failure: mypy builds no
/// call variant for a dunder nothing defines. `None` means `type_ref`, or
/// some MRO ancestor of it, is missing from `resolver`, which the kernel
/// cannot distinguish from "present but unknown".
pub fn lookup_operator_definer(
    resolver: &TypeResolver,
    type_ref: &str,
    attr_name: &str,
) -> Option<OperatorDefiner> {
    let found = lookup_definer(resolver, type_ref, attr_name)?;
    Some(match found {
        Some(fullname) => OperatorDefiner::Found(fullname),
        None => OperatorDefiner::Absent,
    })
}

/// Whether a binary operator's dunder skips the reflected variant when
/// both operands are the same type.
///
/// Lifts `checkoperator::is_shortcut_op`, membership in
/// `mypy.operators.op_methods_that_shortcut` (operators.py:150). Only the
/// forward arithmetic and bitwise dunders are members; reflected
/// (`__radd__`) and comparison (`__lt__`) names never are.
pub fn op_method_shortcuts(op_name: &str) -> bool {
    is_shortcut_op(op_name)
}

/// Whether a comparison dunder falls back to the reflected comparison.
///
/// Lifts `operators::falls_back_to_cmp`, membership in
/// `mypy.operators.ops_falling_back_to_cmp` (operators.py:147): the six
/// rich-comparison dunders, and nothing else.
pub fn op_falls_back_to_cmp(op_name: &str) -> bool {
    falls_back_to_cmp(op_name)
}

/// The reflected dunder mypy pairs with `op_name`.
///
/// Lifts `operators::get_reverse_op_method`, a lookup in
/// `mypy.operators.reverse_op_methods` (operators.py:55). Arithmetic maps
/// to its `__r`-prefixed partner, comparisons map to their mirror
/// (`__lt__` to `__gt__`), and `__eq__`/`__ne__` map to themselves. `None`
/// means `op_name` has no reflected partner at all.
pub fn reverse_op_method(op_name: &str) -> Option<&'static str> {
    get_reverse_op_method(op_name)
}

/// The literal type an expression's type stands for.
///
/// Lifts `checkexpr_functions::try_getting_literal_inner`, the port of
/// `mypy.checkexpr.try_getting_literal` (checkexpr.py:9348): an `Instance`
/// carrying a `last_known_value` yields that literal, and every other
/// proper type yields itself. `None` on a `TypeAliasType`.
pub fn try_getting_literal(typ: &Type) -> Option<Type> {
    try_getting_literal_inner(typ)
}

/// The int literal values a type stands for.
///
/// Lifts `checkexpr_functions::try_getting_int_literals_inner`, the
/// type-level half of `ExpressionChecker.try_getting_int_literals`
/// (mypy/checkexpr.py:6710). A `Literal[int]`, an `Instance` carrying one
/// as its `last_known_value`, or a union of those yields the values in
/// operand order. `None` for any other shape, which is how mypy reports
/// "this is not an int literal" (tuple indexing, slice bounds).
pub fn try_getting_int_literals(typ: &Type) -> Option<Vec<i64>> {
    try_getting_int_literals_inner(typ)
}

/// Whether a type is exactly a string literal.
///
/// Lifts `checkexpr_functions::is_string_literal_inner`, the port of
/// `mypy.checker.is_string_literal` (checker.py:12504). A single-item union
/// is judged by its item. `None` on a `TypeAliasType`.
pub fn is_string_literal(typ: &Type) -> Option<bool> {
    is_string_literal_inner(typ)
}

/// Whether a tuple display's elements fit a tuple type context.
///
/// Lifts `checkexpr_functions::tuple_context_matches_inner`, the port of
/// `ExpressionChecker.tuple_context_matches` (mypy/checkexpr.py:7303).
///
/// `elements_tags` carries mypy's tag per element: `0` for a plain element,
/// `1` for a `*`-starred one, of which there may be several. Against a
/// fixed tuple context the plain-element count must fit; against a
/// variadic context exactly one star must sit where the context's `Unpack`
/// sits. `None` when `ctx` is a `TypeAliasType`, and `Some(false)` when it
/// is not a tuple type at all.
pub fn tuple_context_matches(elements_tags: &[i64], ctx: &Type) -> Option<bool> {
    tuple_context_matches_inner(elements_tags, ctx)
}

/// Whether a declared return type is a generator or coroutine return.
///
/// Lifts `generators::is_generator_return_type`, the port of
/// `TypeChecker.is_generator_return_type` (mypy/checker.py:1422). With
/// `is_coroutine` the probe operand is `Awaitable[Any]`, otherwise
/// `Generator[Any, Any, Any]`; `typing.AwaitableGenerator` also matches by
/// name. `None` on a `TypeAliasType` or a subtype pair the nominal engine
/// cannot judge.
pub fn is_generator_return_type(
    typ: &Type,
    is_coroutine: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    generator_return_type_inner(typ, is_coroutine, strict_optional, resolver)
}

/// Whether a declared return type is an async generator return.
///
/// Lifts `generators::is_async_generator_return_type`, the port of
/// `TypeChecker.is_async_generator_return_type` (mypy/checker.py:1441): a
/// supertype of `AsyncGenerator[Any, Any]`. `None` on a `TypeAliasType` or
/// an unjudgeable subtype pair.
pub fn is_async_generator_return_type(
    typ: &Type,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<bool> {
    async_generator_return_type_inner(typ, strict_optional, resolver)
}

/// The yield type (`ty`) of a generator or coroutine return type.
///
/// Lifts `generators::get_generator_yield_type_inner`, the port of
/// `TypeChecker.get_generator_yield_type` (mypy/checker.py:1454). Note that
/// a return type which is not a generator at all yields
/// `Any(TypeOfAny.from_error)`: that is mypy's own recovery value, not a
/// deferral, so the driver must not read it as success.
pub fn get_generator_yield_type(
    return_type: &Type,
    is_coroutine: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Type> {
    get_generator_yield_type_inner(return_type, is_coroutine, strict_optional, resolver)
}

/// The receive type (`tc`) of a generator or coroutine return type.
///
/// Lifts `generators::get_generator_receive_type_inner`, the port of
/// `TypeChecker.get_generator_receive_type` (mypy/checker.py:1488). A
/// generator that names no receive parameter gets `NoneType`, as in mypy.
pub fn get_generator_receive_type(
    return_type: &Type,
    is_coroutine: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Type> {
    get_generator_receive_type_inner(return_type, is_coroutine, strict_optional, resolver)
}

/// The return type (`tr`) of a generator or coroutine return type.
///
/// Lifts `generators::get_generator_return_type_inner`, the port of
/// `TypeChecker.get_generator_return_type` (mypy/checker.py:1531). Its
/// non-generator guard is `is_generator_return_type` alone, where yield and
/// receive also accept an async generator; the difference is mypy's and is
/// preserved exactly.
pub fn get_generator_return_type(
    return_type: &Type,
    is_coroutine: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Type> {
    get_generator_return_type_inner(return_type, is_coroutine, strict_optional, resolver)
}

/// The return type of a `Coroutine` instance.
///
/// Lifts `generators::get_coroutine_return_type_inner`, the port of
/// `TypeChecker.get_coroutine_return_type` (mypy/checker.py:1523): the
/// third argument of the instance. `None` on a `TypeAliasType` or an
/// instance with fewer than three arguments, which is a caller contract
/// violation (Python only calls this on a `Coroutine`).
pub fn get_coroutine_return_type(return_type: &Type) -> Option<Type> {
    get_coroutine_return_type_inner(return_type)
}

/// One `%` conversion specifier of a printf-style format string.
///
/// The named form of the seven positional fields
/// `checkstrformat::rust_parse_conversion_specifiers` returns, which mirror
/// the attributes mypy's `ConversionSpecifier` carries for a `%` format
/// (checkstrformat.py:309). `start_pos` is a character offset, not a byte
/// offset, matching mypy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintfSpecifier {
    /// The whole specifier as written, for example `"%-10.2f"`.
    pub whole_seq: String,
    /// Character offset of the `%` in the format string.
    pub start_pos: usize,
    /// The mapping key of `"%(name)s"`. `None` when the specifier has no
    /// key. An empty key (`"%()s"`) is `Some("")`, which mypy's
    /// `ConversionSpecifier.has_key()` counts as present because it tests
    /// `key is not None`, not truthiness.
    pub key: Option<String>,
    /// The conversion character, or `"%"` for a literal percent.
    pub conv_type: String,
    pub flags: String,
    /// `"*"` when the width comes from an argument.
    pub width: String,
    /// `".NN"` or `".*"`, without the leading dot only if mypy omits it.
    pub precision: String,
}

/// One `str.format()` or f-string replacement field.
///
/// The named form of the eleven positional fields
/// `checkstrformat::rust_parse_format_value` returns, which mirror the
/// attributes mypy's `ConversionSpecifier` carries for a new-style format
/// (checkstrformat.py:323). Nested fields are flattened into the same list,
/// in the order mypy produces them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatSpecifier {
    /// The raw field text between the braces, for example `"name:d"`.
    pub whole_seq: String,
    /// Character offset of the field in the format string.
    pub start_pos: usize,
    /// The field name or index, when the field has one.
    pub key: Option<String>,
    pub conv_type: String,
    pub flags: String,
    pub width: String,
    pub precision: String,
    /// Everything after the `:` in the field, when there is one.
    pub format_spec: Option<String>,
    /// True when the field matched mypy's `FORMAT_RE_NEW_CUSTOM` rather
    /// than the built-in grammar, so `format_spec` is unchecked text.
    pub non_standard_format_spec: bool,
    /// The `!r` / `!s` / `!a` conversion, with its leading `!`.
    pub conversion: Option<String>,
    /// The key plus any following attribute or index accesses.
    pub field: Option<String>,
}

/// The decomposed format spec of one replacement field: the part after `:`.
///
/// The named form of the nine positional fields
/// `checkstrformat::rust_parse_placeholder_format` returns, matching the
/// `num_spec` groups of mypy's `FORMAT_RE_NEW` (checkstrformat.py:178).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderFormat {
    pub fill: Option<String>,
    pub align: Option<String>,
    pub sign: Option<String>,
    /// The `#` flag was present.
    pub alternate: bool,
    /// The `0` flag was present after any `#`.
    pub zero_pad: bool,
    pub width: String,
    /// The `_` or `,` thousands separator.
    pub grouping: Option<String>,
    /// `".NN"` including the dot.
    pub precision: String,
    pub conv_type: String,
}

/// A malformed format string, one variant per `msg.fail` call in mypy.
///
/// The kernel reports these as the integers 1 to 5
/// (`checkstrformat::ERR_*`); this is the same set, named. Every variant is
/// reported by mypy under `codes.STRING_FORMATTING`, so the diagnostic code
/// is not part of the distinction. The message text lives in
/// `standalone::diag`, not here: this module says which error occurred, the
/// diag area owns how mypy words it (checkstrformat.py:334-364).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatStringError {
    /// `Invalid conversion specifier in format string: unexpected }`
    UnexpectedCloseBrace,
    /// `Invalid conversion specifier in format string: unmatched {`
    UnmatchedOpenBrace,
    /// `Invalid conversion specifier in format string`
    InvalidSpecifier,
    /// `Conversion value must not contain { or }`
    KeyContainsBrace,
    /// `Formatting nesting must be at most two levels deep`
    NestingTooDeep,
}

/// The three flags mypy's `analyze_conversion_specifiers` derives from a
/// `%` format string (checkstrformat.py:896).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecifierAnalysis {
    /// Some specifier takes its width or precision from a `*` argument.
    pub has_star: bool,
    /// Some specifier has a mapping key. This is the value mypy's method
    /// returns on success, and it decides keyed versus positional
    /// replacement checking.
    pub has_key: bool,
    /// Every specifier either has a key or is a literal `%%`.
    pub all_have_keys: bool,
}

/// Why a `%` format string's specifier list is rejected outright.
///
/// The two variants are mypy's two distinct messages
/// (checkstrformat.py:908 and :910), which the kernel collapses into a
/// single `None`; separating them here is what lets the driver name the
/// right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecifierAnalysisError {
    /// A mapping key and a `*` width or precision in the same string:
    /// `string_interpolation_with_star_and_key`.
    StarWithKey,
    /// Some specifiers have a key and some do not:
    /// `string_interpolation_mixing_key_and_non_keys`.
    MixedKeys,
}

/// Maps one of `checkstrformat`'s integer error codes onto
/// [`FormatStringError`], or `None` for code 0, which means success.
fn format_string_error(code: i32) -> Option<FormatStringError> {
    match code {
        ERR_UNEXPECTED_CLOSE => Some(FormatStringError::UnexpectedCloseBrace),
        ERR_UNMATCHED_OPEN => Some(FormatStringError::UnmatchedOpenBrace),
        ERR_INVALID_SPECIFIER => Some(FormatStringError::InvalidSpecifier),
        ERR_KEY_HAS_BRACE => Some(FormatStringError::KeyContainsBrace),
        ERR_NESTING_TOO_DEEP => Some(FormatStringError::NestingTooDeep),
        _ => None,
    }
}

/// The error for a nonzero code the five constants do not cover.
///
/// Unreachable: the kernel only ever returns its own `ERR_*` constants. It
/// falls back to mypy's generic `InvalidSpecifier` message rather than
/// guessing at a more specific complaint, and rather than panicking inside
/// a checker.
fn unknown_format_code(code: i32) -> FormatStringError {
    format_string_error(code).unwrap_or(FormatStringError::InvalidSpecifier)
}

/// Whether a conversion character is numeric.
///
/// Lifts `checkstrformat::rust_is_numeric_format_type`, the port of
/// `mypy.checkstrformat.is_numeric_format_type` (checkstrformat.py:170).
/// `is_new_style` selects the `str.format()` table over the `%` table; they
/// differ on `b`, `n` and `%`.
pub fn is_numeric_format_type(conv_type: &str, is_new_style: bool) -> bool {
    rust_is_numeric_format_type(conv_type, is_new_style)
}

/// The `%` conversion specifiers of a printf-style format string.
///
/// Lifts `checkstrformat::rust_parse_conversion_specifiers`, the port of
/// `mypy.checkstrformat.parse_conversion_specifiers`
/// (checkstrformat.py:309). Never fails: a string with no specifier yields
/// an empty list, and mypy reports a bad specifier later, when it checks the
/// replacement values.
pub fn parse_conversion_specifiers(format_str: &str) -> Vec<PrintfSpecifier> {
    let raw = rust_parse_conversion_specifiers(format_str);
    let mut out = Vec::with_capacity(raw.len());
    for (whole_seq, start_pos, key, conv_type, flags, width, precision) in raw {
        out.push(PrintfSpecifier {
            whole_seq,
            start_pos,
            key,
            conv_type,
            flags,
            width,
            precision,
        });
    }
    out
}

/// The raw, unparsed replacement fields of a `str.format()` template.
///
/// Lifts `checkstrformat::rust_find_non_escaped_targets`, the port of
/// `mypy.checkstrformat.find_non_escaped_targets` (checkstrformat.py:415).
/// Each entry is the field text without its braces plus its character
/// offset. Paired `{{` and `}}` are escapes and produce nothing.
///
/// # Errors
///
/// [`FormatStringError::UnexpectedCloseBrace`] or
/// [`FormatStringError::UnmatchedOpenBrace`], the only two this function can
/// report.
pub fn find_non_escaped_targets(
    format_value: &str,
) -> Result<Vec<(String, usize)>, FormatStringError> {
    let (code, targets) = rust_find_non_escaped_targets(format_value);
    if code != 0 {
        return Err(unknown_format_code(code));
    }
    Ok(targets)
}

/// The parsed replacement fields of a `str.format()` template or f-string.
///
/// Lifts `checkstrformat::rust_parse_format_value`, the port of
/// `mypy.checkstrformat.parse_format_value` (checkstrformat.py:323). Fields
/// nested inside a non-standard format spec are flattened into the same
/// list, at most two levels deep, in mypy's order.
///
/// # Errors
///
/// Any of the five [`FormatStringError`] variants, which are exactly the
/// five `msg.fail` calls mypy makes here.
pub fn parse_format_value(format_value: &str) -> Result<Vec<FormatSpecifier>, FormatStringError> {
    let (code, raw) = rust_parse_format_value(format_value);
    if code != 0 {
        return Err(unknown_format_code(code));
    }
    let mut out = Vec::with_capacity(raw.len());
    for spec in raw {
        let (
            whole_seq,
            start_pos,
            key,
            conv_type,
            flags,
            width,
            precision,
            format_spec,
            non_standard_format_spec,
            conversion,
            field,
        ) = spec;
        out.push(FormatSpecifier {
            whole_seq,
            start_pos,
            key,
            conv_type,
            flags,
            width,
            precision,
            format_spec,
            non_standard_format_spec,
            conversion,
            field,
        });
    }
    Ok(out)
}

/// The decomposed format spec of one replacement field.
///
/// Lifts `checkstrformat::rust_parse_placeholder_format`, the port of
/// `mypy.checkstrformat.parse_placeholder_format` (checkstrformat.py:178).
/// `format_spec` is the text after the `:`, without the colon.
///
/// `None` is mypy's "this does not match the built-in grammar", which sends
/// the field down the custom `__format__` path rather than reporting an
/// error.
pub fn parse_placeholder_format(format_spec: &str) -> Option<PlaceholderFormat> {
    let raw = rust_parse_placeholder_format(format_spec)?;
    let (fill, align, sign, alternate, zero_pad, width, grouping, precision, conv_type) = raw;
    Some(PlaceholderFormat {
        fill,
        align,
        sign,
        alternate,
        zero_pad,
        width,
        grouping,
        precision,
        conv_type,
    })
}

/// Whether a `%` format string's specifiers are usable, and how.
///
/// Lifts `checkstrformat::rust_analyze_conversion_specifiers`, the port of
/// `StringFormatterChecker.analyze_conversion_specifiers`
/// (checkstrformat.py:896). The success value's `has_key` is what mypy
/// returns to pick keyed over positional replacement checking.
///
/// `key_present` is `key.is_some()`, not key non-emptiness: mypy's
/// `ConversionSpecifier.has_key()` is `self.key is not None`
/// (checkstrformat.py:272), so an empty `"%()s"` key counts as present.
///
/// # Errors
///
/// [`SpecifierAnalysisError::StarWithKey`] or
/// [`SpecifierAnalysisError::MixedKeys`]. The kernel declines without saying
/// which, so the two flags are recomputed here exactly as mypy's own Python
/// fallback does (checkstrformat.py:905).
pub fn analyze_conversion_specifiers(
    specifiers: &[PrintfSpecifier],
) -> Result<SpecifierAnalysis, SpecifierAnalysisError> {
    let infos: Vec<(bool, String, String, String)> = specifiers
        .iter()
        .map(|s| {
            (
                s.key.is_some(),
                s.conv_type.clone(),
                s.width.clone(),
                s.precision.clone(),
            )
        })
        .collect();
    if let Some((has_star, has_key, all_have_keys)) = rust_analyze_conversion_specifiers(infos) {
        return Ok(SpecifierAnalysis {
            has_star,
            has_key,
            all_have_keys,
        });
    }
    let has_star = specifiers
        .iter()
        .any(|s| s.width == "*" || s.precision == "*");
    let has_key = specifiers.iter().any(|s| s.key.is_some());
    if has_star && has_key {
        return Err(SpecifierAnalysisError::StarWithKey);
    }
    Err(SpecifierAnalysisError::MixedKeys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtypes::COVARIANT;
    use crate::typeinfo::TypeInfoSnapshot;
    use crate::wire::LiteralValue;

    /// `TypeOfAny.special_form` (mypy/types.py:233).
    const ANY_SPECIAL_FORM: i64 = 6;

    fn instance(type_ref: &str, args: Vec<Type>) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args,
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn instance_with_lkv(type_ref: &str, lkv: Type) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args: Vec::new(),
            last_known_value: Some(Box::new(lkv)),
            extra_attrs: None,
        }
    }

    fn special_form_any() -> Type {
        Type::AnyType {
            type_of_any: ANY_SPECIAL_FORM,
            source_any: None,
            missing_import_name: None,
        }
    }

    /// A `TypeAliasType` with no installed snapshot: the shape every lift
    /// here must decline rather than guess at.
    fn alias() -> Type {
        Type::TypeAliasType {
            args: Vec::new(),
            type_ref: "mod.A".to_string(),
            is_recursive: false,
        }
    }

    fn literal(value: LiteralValue, fallback: &str) -> Type {
        Type::LiteralType {
            fallback: Box::new(instance(fallback, Vec::new())),
            value,
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

    fn tuple(items: Vec<Type>) -> Type {
        Type::TupleType {
            partial_fallback: Box::new(instance("builtins.tuple", vec![special_form_any()])),
            items,
            implicit: false,
        }
    }

    /// A snapshot carrying its own fullname in `mro`/`has_base`, as a real
    /// `TypeInfo` does, plus `tvars` covariant type variables so an
    /// exactly-matching generic `Instance` can be judged nominally.
    fn snap(fullname: &str, name: &str, tvars: usize) -> TypeInfoSnapshot {
        let mut s = TypeInfoSnapshot {
            fullname: fullname.to_string(),
            name: name.to_string(),
            ..Default::default()
        };
        s.mro.push(fullname.to_string());
        s.has_base.insert(fullname.to_string());
        s.type_vars_with_variance = (0..tvars)
            .map(|i| (format!("T{i}"), COVARIANT, 0))
            .collect();
        s
    }

    fn with_member(mut s: TypeInfoSnapshot, name: &str) -> TypeInfoSnapshot {
        s.member_info.insert(name.to_string(), (false, true));
        s
    }

    fn resolver(snaps: Vec<TypeInfoSnapshot>) -> TypeResolver {
        let mut r = TypeResolver::new();
        for s in snaps {
            r.insert(s.fullname.clone(), s);
        }
        r
    }

    /// The snapshots the generator ports probe: `typing.Generator` (3
    /// covariant params), `typing.Awaitable` (1), `typing.AsyncGenerator`
    /// (2) and the builtins a concrete argument list names.
    fn generator_resolver() -> TypeResolver {
        resolver(vec![
            snap("typing.Generator", "Generator", 3),
            snap("typing.Awaitable", "Awaitable", 1),
            snap("typing.AsyncGenerator", "AsyncGenerator", 2),
            snap("builtins.object", "object", 0),
            snap("builtins.int", "int", 0),
            snap("builtins.str", "str", 0),
            snap("builtins.bool", "bool", 0),
        ])
    }

    /// `Generator[int, str, bool]`: the return type a generator
    /// expression's element typing extracts ty, tc and tr from.
    fn concrete_generator() -> Type {
        instance(
            "typing.Generator",
            vec![
                instance("builtins.int", Vec::new()),
                instance("builtins.str", Vec::new()),
                instance("builtins.bool", Vec::new()),
            ],
        )
    }

    // -- check_op_reversible_variant_order --

    #[test]
    fn same_type_shortcut_operands_use_the_forward_variant_alone() {
        let r = resolver(vec![snap("a.A", "A", 0)]);
        let a = instance("a.A", Vec::new());
        assert_eq!(
            check_op_reversible_variant_order("__add__", &a, &a, &r, true),
            Some(OperatorVariantOrder::ShortcutSingle)
        );
    }

    #[test]
    fn unrelated_operands_use_normal_variant_order() {
        // a.A defines __add__ and a.B defines nothing, so the definers
        // differ and covers_at_runtime(B, A) is false. This is the shape
        // checkoperator.rs pins as shortcut_different_instances_normal_order.
        let a = with_member(snap("a.A", "A", 0), "__add__");
        let b = snap("a.B", "B", 0);
        let r = resolver(vec![a, b]);
        let left = instance("a.A", Vec::new());
        let right = instance("a.B", Vec::new());
        assert_eq!(
            check_op_reversible_variant_order("__add__", &left, &right, &r, true),
            Some(OperatorVariantOrder::Normal)
        );
    }

    #[test]
    fn an_any_operand_defers_the_variant_order() {
        // checkexpr.py:4660: Python returns Any before any ordering, so the
        // kernel declines and the driver must reject rather than pick Normal.
        let r = resolver(vec![snap("a.A", "A", 0)]);
        let a = instance("a.A", Vec::new());
        assert_eq!(
            check_op_reversible_variant_order("__add__", &special_form_any(), &a, &r, true),
            None
        );
    }

    // -- lookup_operator_definer --

    #[test]
    fn the_definer_walks_the_mro_to_the_defining_class() {
        let obj = with_member(snap("builtins.object", "object", 0), "__add__");
        let mut a = snap("a.A", "A", 0);
        a.mro.push("builtins.object".to_string());
        let r = resolver(vec![obj, a]);
        assert_eq!(
            lookup_operator_definer(&r, "a.A", "__add__"),
            Some(OperatorDefiner::Found("builtins.object".to_string()))
        );
    }

    #[test]
    fn a_dunder_no_mro_class_defines_is_absent_not_a_deferral() {
        let r = resolver(vec![snap("a.A", "A", 0)]);
        assert_eq!(
            lookup_operator_definer(&r, "a.A", "__radd__"),
            Some(OperatorDefiner::Absent)
        );
    }

    #[test]
    fn a_missing_snapshot_defers_the_definer_lookup() {
        let r = resolver(Vec::new());
        assert_eq!(lookup_operator_definer(&r, "a.Unknown", "__add__"), None);
    }

    // -- the operator tables --

    #[test]
    fn the_shortcut_table_holds_forward_dunders_only() {
        assert!(op_method_shortcuts("__add__"));
        assert!(!op_method_shortcuts("__radd__"));
    }

    #[test]
    fn the_cmp_fallback_table_holds_comparison_dunders_only() {
        assert!(op_falls_back_to_cmp("__lt__"));
        assert!(!op_falls_back_to_cmp("__add__"));
    }

    #[test]
    fn reverse_op_method_mirrors_and_reports_a_missing_partner() {
        assert_eq!(reverse_op_method("__add__"), Some("__radd__"));
        assert_eq!(reverse_op_method("__lt__"), Some("__gt__"));
        assert_eq!(reverse_op_method("__index__"), None);
    }

    // -- literal and container expressions --

    #[test]
    fn try_getting_literal_unwraps_a_last_known_value() {
        let lkv = literal(LiteralValue::Int(3), "builtins.int");
        let t = instance_with_lkv("builtins.int", lkv.clone());
        assert_eq!(try_getting_literal(&t), Some(lkv));
    }

    #[test]
    fn try_getting_literal_defers_on_an_alias() {
        assert_eq!(try_getting_literal(&alias()), None);
    }

    #[test]
    fn int_literals_collect_every_union_member_in_order() {
        let u = union(vec![
            literal(LiteralValue::Int(1), "builtins.int"),
            literal(LiteralValue::Int(2), "builtins.int"),
        ]);
        assert_eq!(try_getting_int_literals(&u), Some(vec![1, 2]));
    }

    #[test]
    fn a_str_literal_yields_no_int_literals() {
        let s = literal(LiteralValue::Str("x".to_string()), "builtins.str");
        assert_eq!(try_getting_int_literals(&s), None);
    }

    #[test]
    fn only_a_str_literal_is_a_string_literal() {
        let s = literal(LiteralValue::Str("x".to_string()), "builtins.str");
        assert_eq!(is_string_literal(&s), Some(true));
        let i = literal(LiteralValue::Int(1), "builtins.int");
        assert_eq!(is_string_literal(&i), Some(false));
    }

    #[test]
    fn is_string_literal_defers_on_an_alias() {
        assert_eq!(is_string_literal(&alias()), None);
    }

    #[test]
    fn a_fixed_tuple_context_fits_when_the_plain_count_fits() {
        let i = instance("builtins.int", Vec::new());
        let ctx = tuple(vec![i.clone(), i]);
        assert_eq!(tuple_context_matches(&[0, 0], &ctx), Some(true));
        assert_eq!(tuple_context_matches(&[0, 0, 0], &ctx), Some(false));
    }

    #[test]
    fn an_alias_context_defers_the_tuple_match() {
        assert_eq!(tuple_context_matches(&[0], &alias()), None);
    }

    // -- generator and comprehension element typing --

    #[test]
    fn a_generator_return_type_is_recognized() {
        let r = generator_resolver();
        let g = concrete_generator();
        assert_eq!(is_generator_return_type(&g, false, true, &r), Some(true));
    }

    #[test]
    fn an_async_generator_return_type_is_recognized() {
        let r = generator_resolver();
        let ag = instance(
            "typing.AsyncGenerator",
            vec![instance("builtins.int", Vec::new()), special_form_any()],
        );
        assert_eq!(is_async_generator_return_type(&ag, true, &r), Some(true));
    }

    #[test]
    fn an_alias_defers_both_generator_classifications() {
        let r = generator_resolver();
        let a = alias();
        assert_eq!(is_generator_return_type(&a, false, true, &r), None);
        assert_eq!(is_async_generator_return_type(&a, true, &r), None);
    }

    #[test]
    fn the_three_generator_parameters_come_out_in_declaration_order() {
        let r = generator_resolver();
        let g = concrete_generator();
        assert_eq!(
            get_generator_yield_type(&g, false, true, &r),
            Some(instance("builtins.int", Vec::new()))
        );
        assert_eq!(
            get_generator_receive_type(&g, false, true, &r),
            Some(instance("builtins.str", Vec::new()))
        );
        assert_eq!(
            get_generator_return_type(&g, false, true, &r),
            Some(instance("builtins.bool", Vec::new()))
        );
    }

    #[test]
    fn an_alias_defers_every_generator_parameter_extraction() {
        let r = generator_resolver();
        let a = alias();
        assert_eq!(get_generator_yield_type(&a, false, true, &r), None);
        assert_eq!(get_generator_receive_type(&a, false, true, &r), None);
        assert_eq!(get_generator_return_type(&a, false, true, &r), None);
    }

    #[test]
    fn the_coroutine_return_type_is_the_third_argument() {
        let c = instance(
            "typing.Coroutine",
            vec![
                special_form_any(),
                special_form_any(),
                instance("builtins.int", Vec::new()),
            ],
        );
        assert_eq!(
            get_coroutine_return_type(&c),
            Some(instance("builtins.int", Vec::new()))
        );
    }

    #[test]
    fn a_coroutine_instance_without_a_third_argument_defers() {
        let c = instance(
            "typing.Coroutine",
            vec![special_form_any(), special_form_any()],
        );
        assert_eq!(get_coroutine_return_type(&c), None);
    }

    // -- format strings --

    #[test]
    fn the_numeric_format_tables_differ_between_the_two_styles() {
        assert!(is_numeric_format_type("d", false));
        assert!(is_numeric_format_type("%", true));
        assert!(!is_numeric_format_type("%", false));
        assert!(!is_numeric_format_type("s", true));
    }

    #[test]
    fn printf_specifiers_keep_the_key_apart_from_the_conversion() {
        let specs = parse_conversion_specifiers("%(name)s");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].whole_seq, "%(name)s");
        assert_eq!(specs[0].start_pos, 0);
        assert_eq!(specs[0].key, Some("name".to_string()));
        assert_eq!(specs[0].conv_type, "s");
    }

    #[test]
    fn a_star_width_stays_a_star_and_a_percent_literal_stays_a_type() {
        let star = parse_conversion_specifiers("%*d");
        assert_eq!(star.len(), 1);
        assert_eq!(star[0].width, "*");
        assert_eq!(star[0].conv_type, "d");
        assert_eq!(star[0].key, None);

        let pct = parse_conversion_specifiers("100%%");
        assert_eq!(pct.len(), 1);
        assert_eq!(pct[0].conv_type, "%");
    }

    #[test]
    fn a_format_string_without_specifiers_parses_to_nothing() {
        assert!(parse_conversion_specifiers("no specifiers here").is_empty());
    }

    #[test]
    fn non_escaped_targets_are_raw_field_text_at_character_offsets() {
        // The offset is the closing brace's index minus the field length, so
        // it points at the first character inside the braces, not at `{`.
        assert_eq!(
            find_non_escaped_targets("{name}"),
            Ok(vec![("name".to_string(), 1)])
        );
        // A multibyte prefix shifts the character offset, which is what mypy
        // reports, not the byte offset.
        assert_eq!(
            find_non_escaped_targets("ā{}"),
            Ok(vec![(String::new(), 2)])
        );
        // Paired braces are escapes and produce no field at all.
        assert_eq!(find_non_escaped_targets("{{}}"), Ok(vec![]));
    }

    #[test]
    fn an_unbalanced_brace_is_a_named_error_not_an_empty_field_list() {
        assert_eq!(
            find_non_escaped_targets("}"),
            Err(FormatStringError::UnexpectedCloseBrace)
        );
        assert_eq!(
            find_non_escaped_targets("{"),
            Err(FormatStringError::UnmatchedOpenBrace)
        );
    }

    #[test]
    fn a_new_style_field_decomposes_into_key_conversion_and_spec() {
        let specs = parse_format_value("{name:d}").expect("a builtin field must parse");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].whole_seq, "name:d");
        assert_eq!(specs[0].key, Some("name".to_string()));
        assert_eq!(specs[0].conv_type, "d");
        assert!(!specs[0].non_standard_format_spec);
    }

    #[test]
    fn a_bang_conversion_keeps_its_bang() {
        let specs = parse_format_value("{!r}").expect("a builtin field must parse");
        assert_eq!(specs[0].conversion, Some("!r".to_string()));
    }

    #[test]
    fn a_malformed_template_names_its_error_instead_of_parsing() {
        assert_eq!(
            parse_format_value("}"),
            Err(FormatStringError::UnexpectedCloseBrace)
        );
        assert_eq!(
            parse_format_value("{"),
            Err(FormatStringError::UnmatchedOpenBrace)
        );
    }

    #[test]
    fn every_mypy_format_error_code_maps_to_its_own_variant() {
        assert_eq!(
            format_string_error(ERR_UNEXPECTED_CLOSE),
            Some(FormatStringError::UnexpectedCloseBrace)
        );
        assert_eq!(
            format_string_error(ERR_UNMATCHED_OPEN),
            Some(FormatStringError::UnmatchedOpenBrace)
        );
        assert_eq!(
            format_string_error(ERR_INVALID_SPECIFIER),
            Some(FormatStringError::InvalidSpecifier)
        );
        assert_eq!(
            format_string_error(ERR_KEY_HAS_BRACE),
            Some(FormatStringError::KeyContainsBrace)
        );
        assert_eq!(
            format_string_error(ERR_NESTING_TOO_DEEP),
            Some(FormatStringError::NestingTooDeep)
        );
        assert_eq!(format_string_error(0), None);
        assert_eq!(unknown_format_code(99), FormatStringError::InvalidSpecifier);
    }

    #[test]
    fn a_placeholder_spec_decomposes_into_all_nine_parts() {
        let p = parse_placeholder_format("x<+#012,.3f").expect("a builtin spec must parse");
        assert_eq!(p.fill, Some("x".to_string()));
        assert_eq!(p.align, Some("<".to_string()));
        assert_eq!(p.sign, Some("+".to_string()));
        assert!(p.alternate);
        assert!(p.zero_pad);
        assert_eq!(p.width, "12");
        assert_eq!(p.grouping, Some(",".to_string()));
        assert_eq!(p.precision, ".3");
        assert_eq!(p.conv_type, "f");
    }

    #[test]
    fn a_spec_outside_the_builtin_grammar_is_not_a_builtin_spec() {
        assert_eq!(parse_placeholder_format("d extra"), None);
    }

    #[test]
    fn keyed_specifiers_analyze_as_keyed() {
        let specs = parse_conversion_specifiers("%(a)s %(b)d");
        assert_eq!(
            analyze_conversion_specifiers(&specs),
            Ok(SpecifierAnalysis {
                has_star: false,
                has_key: true,
                all_have_keys: true,
            })
        );
    }

    #[test]
    fn positional_specifiers_analyze_as_unkeyed() {
        let specs = parse_conversion_specifiers("%d %s");
        assert_eq!(
            analyze_conversion_specifiers(&specs),
            Ok(SpecifierAnalysis {
                has_star: false,
                has_key: false,
                all_have_keys: false,
            })
        );
    }

    #[test]
    fn a_star_beside_a_key_is_rejected_as_star_with_key() {
        let specs = parse_conversion_specifiers("%(a)*d");
        assert_eq!(
            analyze_conversion_specifiers(&specs),
            Err(SpecifierAnalysisError::StarWithKey)
        );
    }

    #[test]
    fn mixing_keyed_and_unkeyed_specifiers_is_rejected_as_mixed_keys() {
        let specs = parse_conversion_specifiers("%(a)s %d");
        assert_eq!(
            analyze_conversion_specifiers(&specs),
            Err(SpecifierAnalysisError::MixedKeys)
        );
    }
}
