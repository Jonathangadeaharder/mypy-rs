//! Standalone driver glue: expr.
//!
//! Drive expression checking over the lowered expression tree.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::expr`. It owns no checking logic of its own:
//! it adapts skeleton records (`crate::model`, `crate::fixtures`) to the
//! kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `expr` lane for this wave. Add `#[cfg(test)]`
//! unit tests here.
//!
//! # What "adapting" means here, and where the line is
//!
//! Two directions and nothing else:
//!
//! - Down: a skeleton `BinOpKind` / `UnaryOpKind`, the operand `Type`s and
//!   the `TypeResolver` that `crate::fixtures::Fixtures::load` builds
//!   become the arguments `type_kernel::standalone::expr` asks for. The
//!   dunder names are looked up, never decided: the binary ones come from
//!   `subset::BinOpKind::dunders`, the unary ones from
//!   `mypy.operators.unary_op_methods`.
//! - Up: the kernel's `None` ("I decline to decide") becomes
//!   [`Unsupported`], which names the construct. The driver renders that as
//!   an out-of-subset error and exits nonzero. A declined decision is never
//!   turned into a default answer, because on the standalone path there is
//!   no Python body behind it to be right instead.
//!
//! Nothing here is wired into `Driver` yet. Wave 2 replaces the private
//! variant engine in `check.rs::binop_typed` and the dunder table inlined in
//! `check.rs::type_unary` with calls into this module.
//!
//! # Format strings
//!
//! The `%` and `str.format()` entry points take a `subset::Lit`, because that
//! is what the lowered tree hands the driver: `"..." % x` is an
//! `ExprKind::BinOp` with `Mod` and a `Lit::Str` left operand, and
//! `"...".format(x)` is an `ExprKind::Call` whose `func` is an
//! `ExprKind::Attr` over a `Lit::Str`. A non-string template is not an error
//! here; it is the case where mypy has nothing to parse at check time.
//! F-strings are not representable in the current subset at all (it has no
//! joined-string node), so growing the subset comes before wiring them.
//!
//! Error text stays out of this module too. A malformed template is reported
//! as a `FormatStringError` variant, and turning that variant into mypy's
//! message is `port::diag`'s job.

use type_kernel::skeleton_api::Type;
use type_kernel::skeleton_api::TypeResolver;
use type_kernel::standalone::expr::check_op_reversible_variant_order;
use type_kernel::standalone::expr::lookup_operator_definer;
use type_kernel::standalone::expr::parse_conversion_specifiers;
use type_kernel::standalone::expr::parse_format_value;
use type_kernel::standalone::expr::FormatSpecifier;
use type_kernel::standalone::expr::FormatStringError;
use type_kernel::standalone::expr::OperatorDefiner;
use type_kernel::standalone::expr::OperatorVariantOrder;
use type_kernel::standalone::expr::PrintfSpecifier;

use crate::subset::BinOpKind;
use crate::subset::Lit;
use crate::subset::UnaryOpKind;

/// An expression the standalone path cannot type, because the kernel
/// declined the decision and there is no Python fallback behind it.
///
/// The driver renders this as an out-of-subset error. `construct` is the
/// name mypy gives the construct, so the message says what was rejected
/// rather than only that something was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// The rejected construct, named the way mypy names it.
    pub construct: &'static str,
    /// The specific fact that could not be decided.
    pub detail: String,
}

impl Unsupported {
    fn declined(construct: &'static str, detail: &str) -> Self {
        Unsupported {
            construct,
            detail: detail.to_string(),
        }
    }
}

/// The dunder calling order mypy uses for one skeleton binary operator.
///
/// The forward dunder comes from `op.dunders()`, which is the skeleton's
/// copy of `mypy.operators.op_methods[e.op]` as read at
/// checkexpr.py:5342; the reflected name is derived inside the kernel. The
/// answer is the order only. Trying each variant against the operand types
/// and rendering `Unsupported operand types for ...` stays with the driver,
/// as it does in mypy.
///
/// # Errors
///
/// [`Unsupported`] when the kernel declines the ordering: an `Any` operand
/// (mypy returns `Any` before any ordering), an operand type it cannot
/// expand, or a class missing from `resolver`.
pub fn binop_variant_order(
    op: BinOpKind,
    left: &Type,
    right: &Type,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Result<OperatorVariantOrder, Unsupported> {
    let (forward, _) = op.dunders();
    match check_op_reversible_variant_order(forward, left, right, resolver, strict_optional) {
        Some(order) => Ok(order),
        None => Err(Unsupported::declined(
            "binary operator",
            &format!(
                "the kernel declined the `{forward}` variant order, so this operand pair is \
                 outside the standalone subset"
            ),
        )),
    }
}

/// The class that defines one operator dunder for one operand.
///
/// The operand half of `ExpressionChecker.lookup_definer`
/// (mypy/checkexpr.py:5874), reached through
/// `type_kernel::standalone::expr::lookup_operator_definer`.
/// [`OperatorDefiner::Absent`] is an answer: mypy builds no call variant
/// for a dunder nothing defines.
///
/// # Errors
///
/// [`Unsupported`] when `operand` is not an `Instance`, or when the kernel
/// declines the MRO walk because `operand`'s class or one of its ancestors
/// is missing from `resolver`.
pub fn operand_definer(
    dunder: &str,
    operand: &Type,
    resolver: &TypeResolver,
) -> Result<OperatorDefiner, Unsupported> {
    let Type::Instance { type_ref, .. } = operand else {
        return Err(Unsupported::declined(
            "operator operand",
            "the operand is not an instance type, so no operator dunder can be located for it",
        ));
    };
    match lookup_operator_definer(resolver, type_ref, dunder) {
        Some(definer) => Ok(definer),
        None => Err(Unsupported::declined(
            "operator operand",
            &format!(
                "`{type_ref}` or one of its MRO ancestors is missing from the resolver, so \
                 `{dunder}` cannot be located"
            ),
        )),
    }
}

/// The dunder mypy resolves a skeleton unary operator through.
///
/// `mypy.checkexpr.visit_unary_expr` (checkexpr.py:6327) looks the operator
/// symbol up in `mypy.operators.unary_op_methods` (operators.py:103), which
/// holds `-`, `+` and `~`. `not` is not in that table: mypy types `not x`
/// as `builtins.bool` for any operand and never consults a dunder, so `None`
/// here means "no dunder applies", not "unsupported".
pub fn unary_op_dunder(op: UnaryOpKind) -> Option<&'static str> {
    match op {
        UnaryOpKind::USub => Some("__neg__"),
        UnaryOpKind::UAdd => Some("__pos__"),
        UnaryOpKind::Not => None,
    }
}

/// What a skeleton literal used as a `str.format()` template turns out to be.
///
/// The three cases have three different consequences for the driver, which
/// is why they are variants and not an `Option<Result<..>>`: only
/// [`FormatTemplate::Fields`] has replacement values to check,
/// [`FormatTemplate::Malformed`] is one `string-formatting` diagnostic and no
/// checking, and [`FormatTemplate::NotALiteral`] is neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatTemplate {
    /// The replacement fields mypy would check, in source order.
    Fields(Vec<FormatSpecifier>),
    /// The template is malformed. mypy reports one error and checks no
    /// replacement values at all.
    Malformed(FormatStringError),
    /// The template is not a string literal, so there is nothing to parse at
    /// check time.
    NotALiteral,
}

/// The `%` specifiers of a skeleton interpolation's template.
///
/// Adapts a `subset::Lit` to
/// `type_kernel::standalone::expr::parse_conversion_specifiers`, the port of
/// `mypy.checkstrformat.parse_conversion_specifiers`. Parsing never fails: a
/// template with no specifier yields an empty list, and mypy reports a bad
/// specifier later, while checking the replacement values.
///
/// `None` when the template is not a string literal, which is mypy's
/// runtime-checked-format case rather than an error.
pub fn percent_specifiers(template: &Lit) -> Option<Vec<PrintfSpecifier>> {
    match template {
        Lit::Str(text) => Some(parse_conversion_specifiers(text)),
        _ => None,
    }
}

/// The replacement fields of a skeleton `str.format()` template.
///
/// Adapts a `subset::Lit` to
/// `type_kernel::standalone::expr::parse_format_value`, the port of
/// `mypy.checkstrformat.parse_format_value`. See [`FormatTemplate`] for why
/// the three outcomes are variants.
pub fn format_template(template: &Lit) -> FormatTemplate {
    let Lit::Str(text) = template else {
        return FormatTemplate::NotALiteral;
    };
    match parse_format_value(text) {
        Ok(fields) => FormatTemplate::Fields(fields),
        Err(err) => FormatTemplate::Malformed(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    /// `TypeOfAny.special_form` (mypy/types.py:233). The kernel declines
    /// every ordering question about an `Any`, which is what these tests
    /// use it for.
    fn any_operand() -> Type {
        Type::AnyType {
            type_of_any: 6,
            source_any: None,
            missing_import_name: None,
        }
    }

    /// `builtins.object` as a skeleton class model, so an MRO walk that
    /// reaches the root finds a snapshot instead of declining.
    fn object_model() -> model::ClassModel {
        model::ClassModel {
            fullname: "builtins.object".to_string(),
            tvars: Vec::new(),
            bases: Vec::new(),
            mro: vec!["builtins.object".to_string()],
            members: std::collections::BTreeMap::new(),
        }
    }

    /// A corpus class defining exactly `dunders`, with `builtins.object`
    /// behind it in the MRO.
    fn class_model(fullname: &str, dunders: &[&str]) -> model::ClassModel {
        let mut members = std::collections::BTreeMap::new();
        for dunder in dunders {
            members.insert(
                (*dunder).to_string(),
                model::Member::Method(model::Sig {
                    params: vec![("other".to_string(), model::object_type())],
                    ret: model::instance(fullname, Vec::new()),
                }),
            );
        }
        model::ClassModel {
            fullname: fullname.to_string(),
            tvars: Vec::new(),
            bases: vec![("builtins.object".to_string(), Vec::new())],
            mro: vec![fullname.to_string(), "builtins.object".to_string()],
            members,
        }
    }

    /// The kernel resolver these models produce: the same shape
    /// `crate::fixtures::Fixtures::load` hands the driver, built here from
    /// `crate::model` records so the test does not depend on fixture
    /// contents it does not own.
    fn resolver(models: Vec<model::ClassModel>) -> TypeResolver {
        let mut r = TypeResolver::new();
        for m in &models {
            model::refresh_snapshot(m, &mut r).expect("a skeleton model must encode");
        }
        r
    }

    fn operand_resolver() -> TypeResolver {
        resolver(vec![object_model(), class_model("a.A", &["__add__"])])
    }

    #[test]
    fn a_same_type_addition_shortcuts_to_the_forward_variant() {
        let r = operand_resolver();
        let a = model::instance("a.A", Vec::new());
        assert_eq!(
            binop_variant_order(BinOpKind::Add, &a, &a, &r, true),
            Ok(OperatorVariantOrder::ShortcutSingle)
        );
    }

    #[test]
    fn a_declined_variant_order_is_rejected_rather_than_defaulted() {
        let r = operand_resolver();
        let a = model::instance("a.A", Vec::new());
        let rejected = binop_variant_order(BinOpKind::Add, &any_operand(), &a, &r, true);
        let err = rejected.expect_err("an Any operand must not get a variant order");
        assert_eq!(err.construct, "binary operator");
        assert!(err.detail.contains("__add__"), "{}", err.detail);
    }

    #[test]
    fn the_definer_of_a_dunder_is_the_class_that_declares_it() {
        let r = operand_resolver();
        let a = model::instance("a.A", Vec::new());
        assert_eq!(
            operand_definer("__add__", &a, &r),
            Ok(OperatorDefiner::Found("a.A".to_string()))
        );
    }

    #[test]
    fn a_dunder_nothing_declares_is_absent_not_an_error() {
        let r = operand_resolver();
        let a = model::instance("a.A", Vec::new());
        assert_eq!(
            operand_definer("__radd__", &a, &r),
            Ok(OperatorDefiner::Absent)
        );
    }

    #[test]
    fn a_non_instance_operand_is_rejected_by_name() {
        let r = operand_resolver();
        let rejected = operand_definer("__add__", &any_operand(), &r);
        let err = rejected.expect_err("an Any operand has no MRO to walk");
        assert_eq!(err.construct, "operator operand");
    }

    #[test]
    fn the_unary_dunder_table_matches_mypy_and_excludes_not() {
        assert_eq!(unary_op_dunder(UnaryOpKind::USub), Some("__neg__"));
        assert_eq!(unary_op_dunder(UnaryOpKind::UAdd), Some("__pos__"));
        assert_eq!(unary_op_dunder(UnaryOpKind::Not), None);
    }

    #[test]
    fn a_percent_template_yields_its_specifiers() {
        let template = Lit::Str("%(a)s %d".to_string());
        let specs = percent_specifiers(&template).expect("a string literal is a template");
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].key, Some("a".to_string()));
        assert_eq!(specs[1].key, None);
        assert_eq!(specs[1].conv_type, "d");
    }

    #[test]
    fn a_non_string_left_operand_is_not_a_percent_template() {
        assert_eq!(percent_specifiers(&Lit::Int(3)), None);
    }

    #[test]
    fn a_format_template_yields_its_replacement_fields() {
        let template = Lit::Str("{name:d}".to_string());
        let fields = match format_template(&template) {
            FormatTemplate::Fields(fields) => fields,
            other => panic!("expected replacement fields, got {other:?}"),
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, Some("name".to_string()));
        assert_eq!(fields[0].conv_type, "d");
    }

    #[test]
    fn a_malformed_template_is_reported_rather_than_silently_emptied() {
        let template = Lit::Str("}".to_string());
        assert_eq!(
            format_template(&template),
            FormatTemplate::Malformed(FormatStringError::UnexpectedCloseBrace)
        );
    }

    #[test]
    fn a_non_string_template_is_neither_fields_nor_malformed() {
        assert_eq!(format_template(&Lit::Int(3)), FormatTemplate::NotALiteral);
    }
}
