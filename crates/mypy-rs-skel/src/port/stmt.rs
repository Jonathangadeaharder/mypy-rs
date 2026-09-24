//! Standalone driver glue: stmt.
//!
//! Drive statement checking pass by pass over the lowered module.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::stmt`. It owns no checking logic of its own:
//! it adapts skeleton records (`crate::model`, `crate::fixtures`) to the
//! kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `stmt` lane for this wave. Add `#[cfg(test)]`
//! unit tests here.
//!
//! # What this module is for
//!
//! The kernel's standalone API takes the same scalar facts the hybrid's
//! seams read off live Python AST nodes, as explicit records. Something
//! has to derive those facts from the skeleton's own lowered AST, and
//! something has to turn the kernel's decisions back into mypy-format
//! diagnostics. Both are adaptation, not checking: every decision here is
//! the kernel's and every message string is mypy's.
//!
//! The wave-2 integration lane wires these functions into
//! `crate::check::Driver::check_main`, replacing the inline statement arms
//! of `Driver::check_seq`. Until then nothing in the binary calls them,
//! which is why `mod port` carries `#[allow(dead_code)]`.

use type_kernel::skeleton_api::{is_subtype, SubtypeContext, Type, TypeResolver};
use type_kernel::standalone::stmt as kernel;

use crate::check::{CheckError, Diagnostic};
use crate::model::Sig;
use crate::subset::BinOpKind;

/// The caller-side state every statement adapter borrows.
///
/// `crate::check::Driver` owns all four fields; the adapter holds none of
/// them, so it cannot drift from the driver's view of the file.
pub struct StmtContext<'a> {
    /// The file being checked, used to prefix out-of-subset rejections.
    pub path: &'a str,
    /// Only the main file renders diagnostics. An imported sibling that
    /// would produce one rejects instead, so the renderer keeps a single
    /// path, the rule `crate::check::Driver::incompatible_assignment`
    /// already applies.
    pub is_main: bool,
    /// The subtype settings the kernel's `is_subtype` consults.
    pub subtype: &'a SubtypeContext,
    /// The class-fact store the kernel's `is_subtype` consults.
    pub resolver: &'a TypeResolver,
}

/// Reads a member off a class snapshot: the standalone substitute for the
/// `TypeInfo.has_readable_member` call the hybrid makes on a live Python
/// `TypeInfo` (mypy/checker.py:11505-11512).
///
/// The hybrid passes a Python object here; the standalone path passes a
/// trait object so the caller can answer from `crate::model::ClassModel`,
/// from a kernel `TypeResolver` snapshot, or from anything else that holds
/// the member facts.
pub trait MemberProbe {
    /// Whether the class `type_ref` has a readable member `name`.
    fn has_readable_member(&self, type_ref: &str, name: &str) -> bool;
}

/// The subset rejection every adapter shares, byte-identical to the format
/// `crate::check::input` uses.
fn out_of_subset(path: &str, line: usize, detail: &str) -> CheckError {
    CheckError::Input(format!("{path}:{line}: skeleton subset error: {detail}"))
}

/// mypy's `TypeStrVisitor` prints builtins types without the `builtins.`
/// prefix and every other fullname verbatim.
fn display_type(fullname: &str) -> &str {
    fullname.strip_prefix("builtins.").unwrap_or(fullname)
}

/// The `type_ref` of a builtins `Instance`; `None` for any other shape.
///
/// The rendered assignment and return error classes the skeleton supports
/// are the builtins primitives the fixtures cover, so a pair outside them
/// is out of subset rather than silently mis-rendered.
fn builtins_ref(typ: &Type) -> Option<&str> {
    match typ {
        Type::Instance { type_ref, .. } if type_ref.starts_with("builtins.") => Some(type_ref),
        _ => None,
    }
}

/// mypy's INCOMPATIBLE_RETURN_VALUE_TYPE as `check_subtype` renders it
/// (mypy/checker.py:7130-7137, `subtype_label="got"`,
/// `supertype_label="expected"`), or `None` when either side is outside the
/// fixture-covered primitives.
fn return_message(got: &Type, expected: &Type) -> Option<String> {
    let got_ref = builtins_ref(got)?;
    let expected_ref = builtins_ref(expected)?;
    Some(format!(
        "error: Incompatible return value type (got \"{got}\", expected \"{expected}\")  \
         [return-value]",
        got = display_type(got_ref),
        expected = display_type(expected_ref),
    ))
}

/// mypy's INCOMPATIBLE_TYPES_IN_ASSIGNMENT as `check_subtype` renders it,
/// or `None` when either side is outside the fixture-covered primitives.
fn assignment_message(value: &Type, declared: &Type) -> Option<String> {
    let value_ref = builtins_ref(value)?;
    let declared_ref = builtins_ref(declared)?;
    Some(format!(
        "error: Incompatible types in assignment (expression has type \
         \"{expr}\", variable has type \"{var}\")  [assignment]",
        expr = display_type(value_ref),
        var = display_type(declared_ref),
    ))
}

impl<'a> StmtContext<'a> {
    /// The kernel's `is_subtype` with this context's settings. A `None`
    /// verdict means the kernel declined, which for a covered file means
    /// the fixture closure broke: an internal error, never a pass.
    fn sub(&self, left: &Type, right: &Type, line: usize) -> Result<bool, CheckError> {
        let path = self.path;
        match is_subtype(left, right, self.subtype, self.resolver) {
            Some(verdict) => Ok(verdict),
            None => Err(CheckError::Internal(format!(
                "{path}:{line}: skeleton internal error: the kernel deferred a subtype \
                 check; the fixture closure no longer covers the corpus"
            ))),
        }
    }

    /// Render `message` for the main file, or reject an imported sibling
    /// that would have produced it.
    fn rendered(
        &self,
        line: usize,
        col: usize,
        message: String,
        sibling_detail: &str,
    ) -> Result<Option<Diagnostic>, CheckError> {
        if self.is_main {
            return Ok(Some(Diagnostic { line, col, message }));
        }
        Err(out_of_subset(self.path, line, sibling_detail))
    }
}

/// The `check_return_stmt` facts the skeleton subset can supply.
///
/// The subset has no `async def`, no generator, no coroutine and no lambda,
/// and every definition it checks carries a return annotation, so those
/// flags are structurally false and `in_checked_function` is structurally
/// true. `declared_none_return` is the one fact that varies, and mypy
/// derives it from the definition's annotation. `warn_return_any` stays
/// false because the standalone path has no `Options` surface: ADR-0008
/// keeps the hybrid's option plumbing off this path, so the flag is a
/// documented constant rather than a silently dropped one.
pub fn return_stmt_facts(return_type: &Type) -> kernel::ReturnStmtFacts {
    kernel::ReturnStmtFacts {
        declared_none_return: matches!(return_type, Type::NoneType),
        in_checked_function: true,
        ..kernel::ReturnStmtFacts::default()
    }
}

/// `mypy/checker.py::TypeChecker.check_return_stmt` (checker.py:6441-6627)
/// plus its `check_subtype` tail (checker.py:7130-7137), driven from the
/// skeleton's records.
///
/// `enclosing` is the definition the statement sits in. `None` means the
/// `return` is at module level, which mypy's semantic analysis reports as
/// `"return" outside function` (mypy/semanal.py:6856) and the skeleton
/// rejects as out of subset, since its lowering never produces one.
/// `returned` is `None` for a bare `return`, otherwise the already-typed
/// expression. `col` is the 1-based column mypy anchors the message at,
/// which for a returned value is the expression's own column.
pub fn check_return(
    ctx: &StmtContext<'_>,
    enclosing: Option<&Sig>,
    returned: Option<&Type>,
    line: usize,
    col: usize,
) -> Result<Option<Diagnostic>, CheckError> {
    let Some(sig) = enclosing else {
        return Err(out_of_subset(
            ctx.path,
            line,
            "a return statement outside a function",
        ));
    };
    let facts = return_stmt_facts(&sig.ret);
    match kernel::check_return_stmt(returned, &sig.ret, &facts) {
        kernel::ReturnOutcome::CheckSubtype => {
            let Some(got) = returned else {
                let path = ctx.path;
                return Err(CheckError::Internal(format!(
                    "{path}:{line}: skeleton internal error: the kernel asked for a \
                     returned-value subtype check on an empty return"
                )));
            };
            if ctx.sub(got, &sig.ret, line)? {
                return Ok(None);
            }
            let Some(message) = return_message(got, &sig.ret) else {
                return Err(out_of_subset(
                    ctx.path,
                    line,
                    "return-value incompatibility is outside the supported error classes",
                ));
            };
            ctx.rendered(
                line,
                col,
                message,
                "return-value incompatibility is outside the supported error classes",
            )
        }
        kernel::ReturnOutcome::EmptyOk
        | kernel::ReturnOutcome::AnyReturnOk
        | kernel::ReturnOutcome::DeclaredNoneOk => Ok(None),
        kernel::ReturnOutcome::EmptyFail => ctx.rendered(
            line,
            col,
            String::from("error: Return value expected  [return-value]"),
            "a missing return value is outside the supported error classes",
        ),
        kernel::ReturnOutcome::DeclaredNoneFail => ctx.rendered(
            line,
            col,
            String::from("error: No return value expected  [return-value]"),
            "a returned value in a None function is outside the supported error classes",
        ),
        // Unreachable for the facts `return_stmt_facts` supplies: the subset
        // has no async generators and `warn_return_any` is off. Rejecting
        // loudly is the plan's rule-6 behaviour if that ever changes.
        kernel::ReturnOutcome::AsyncGeneratorFail | kernel::ReturnOutcome::WarnReturnAny => {
            Err(out_of_subset(
                ctx.path,
                line,
                "a return outside the supported outcomes",
            ))
        }
    }
}

/// `mypy/checker.py::TypeChecker.check_assignment` (checker.py:4681) for the
/// skeleton's single-target assignment, routed through
/// `check_simple_assignment` (checker.py:6325-6436) and its `check_subtype`
/// tail.
///
/// `declared` is the target's declared or already-inferred type and `value`
/// the typed right-hand side. `facts` is the caller's
/// [`kernel::SimpleAssignmentFacts`], which the skeleton derives from
/// whether the file is a stub and whether the right-hand side is an
/// ellipsis. `col` is the 1-based column of the right-hand side, where mypy
/// anchors INCOMPATIBLE_TYPES_IN_ASSIGNMENT.
pub fn check_annotated_assignment(
    ctx: &StmtContext<'_>,
    declared: &Type,
    value: &Type,
    facts: &kernel::SimpleAssignmentFacts,
    line: usize,
    col: usize,
) -> Result<Option<Diagnostic>, CheckError> {
    let Some(arm) = kernel::check_simple_assignment(Some(declared), facts) else {
        return Err(out_of_subset(
            ctx.path,
            line,
            "an assignment target whose type is an unexpandable alias",
        ));
    };
    match arm {
        // A stub `x: T = ...` initializer is accepted without a subtype
        // check (checker.py:6337-6340).
        kernel::SimpleAssignment::StubEllipsis => Ok(None),
        kernel::SimpleAssignment::Direct => {
            if ctx.sub(value, declared, line)? {
                return Ok(None);
            }
            let Some(message) = assignment_message(value, declared) else {
                return Err(out_of_subset(
                    ctx.path,
                    line,
                    "assignment incompatibility is outside the supported error classes",
                ));
            };
            ctx.rendered(
                line,
                col,
                message,
                "assignment incompatibility is outside the supported error classes",
            )
        }
        // Both fallback arms re-infer the right-hand side in the target's
        // type context before the subtype check. The skeleton receives an
        // already-typed value, so it cannot represent the re-inference.
        kernel::SimpleAssignment::FallbackNoPreferred
        | kernel::SimpleAssignment::FallbackLvaluePreferred => Err(out_of_subset(
            ctx.path,
            line,
            "an assignment that re-infers its value in a union target context",
        )),
    }
}

/// The lvalue facts of the skeleton's plain-name targets: a module
/// `x = ...` / `x: T = ...` or a body local of the same shape. mypy sees a
/// `NameExpr` whose node is a `Var`, and `is_definition` is whether this
/// statement is the name's first binding.
pub fn name_lvalue_facts(is_definition: bool) -> kernel::LvalueFacts {
    kernel::LvalueFacts {
        is_definition,
        is_name: true,
        node_is_var: true,
        ..kernel::LvalueFacts::default()
    }
}

/// The lvalue facts of the skeleton's `self.<attr> = <value>` target: mypy
/// sees a `MemberExpr` that is not a definition, so `check_lvalue` routes it
/// to `check_member_assignment`.
pub fn self_attr_lvalue_facts() -> kernel::LvalueFacts {
    kernel::LvalueFacts {
        is_member: true,
        ..kernel::LvalueFacts::default()
    }
}

/// `mypy/operators.py::ops_with_inplace_method` (operators.py:37-51): the
/// operator spellings that admit an in-place dunder. Ported from the Python
/// set because the kernel receives only the membership flag, never the
/// table.
pub fn op_has_inplace_method(symbol: &str) -> bool {
    matches!(
        symbol,
        "+" | "-" | "*" | "/" | "%" | "//" | "**" | "@" | "&" | "|" | "^" | "<<" | ">>"
    )
}

/// `mypy/checker.py::TypeChecker.infer_operator_assignment_method`
/// (checker.py:11498-11520) for an augmented assignment over the skeleton's
/// operator set. Returns `(inplace, method)`: the method name mypy looks up,
/// and whether it is the in-place variant.
///
/// The hybrid reads `has_readable_member` off a live `TypeInfo` and unwraps
/// a `TypedDictType` through its fallback first. The skeleton subset has no
/// TypedDict, so a non-`Instance` target answers `false`, matching the
/// pyfunction's `(False, method)` arm for `AnyType` / `CallableType`.
pub fn operator_assignment_method(
    op: BinOpKind,
    target: &Type,
    probe: &dyn MemberProbe,
) -> (bool, String) {
    let method = op.dunders().0;
    let in_ops = op_has_inplace_method(op.symbol());
    let inplace_name = kernel::inplace_method_name(method);
    let has_inplace = match target {
        Type::Instance { type_ref, .. } => probe.has_readable_member(type_ref, &inplace_name),
        _ => false,
    };
    kernel::infer_operator_assignment_method(in_ops, has_inplace, method)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::instance;

    /// A member probe over a fixed table, standing in for the class-model
    /// lookup the wave-2 driver supplies.
    struct TableProbe {
        members: Vec<(&'static str, &'static str)>,
    }

    impl MemberProbe for TableProbe {
        fn has_readable_member(&self, type_ref: &str, name: &str) -> bool {
            self.members
                .iter()
                .any(|(owner, member)| *owner == type_ref && *member == name)
        }
    }

    struct Harness {
        resolver: TypeResolver,
        subtype: SubtypeContext,
    }

    impl Harness {
        fn new() -> Harness {
            let fixtures = crate::fixtures::Fixtures::load()
                .expect("the fixtures must parse");
            Harness {
                resolver: fixtures.resolver,
                subtype: SubtypeContext {
                    strict_optional: true,
                    ..SubtypeContext::default()
                },
            }
        }

        fn ctx(&self, is_main: bool) -> StmtContext<'_> {
            StmtContext {
                path: "main.py",
                is_main,
                subtype: &self.subtype,
                resolver: &self.resolver,
            }
        }
    }

    fn sig(returning: Type) -> Sig {
        Sig {
            params: Vec::new(),
            ret: returning,
        }
    }

    fn none_sig() -> Sig {
        sig(Type::NoneType)
    }

    // -- check_return -------------------------------------------------------

    #[test]
    fn a_matching_return_value_is_accepted() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let def = sig(instance("builtins.int", Vec::new()));
        let got = instance("builtins.int", Vec::new());
        let diag = check_return(&ctx, Some(&def), Some(&got), 3, 12)
            .expect("decidable");
        assert!(diag.is_none());
    }

    #[test]
    fn a_violating_return_value_renders_the_mypy_message() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let def = sig(instance("builtins.int", Vec::new()));
        let got = instance("builtins.str", Vec::new());
        let diag = check_return(&ctx, Some(&def), Some(&got), 3, 12)
            .expect("decidable")
            .expect("a diagnostic");
        assert_eq!(diag.line, 3);
        assert_eq!(diag.col, 12);
        assert_eq!(
            diag.message,
            "error: Incompatible return value type (got \"str\", expected \"int\")  \
             [return-value]"
        );
    }

    #[test]
    fn a_return_outside_a_function_is_rejected() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let got = instance("builtins.int", Vec::new());
        let err = check_return(&ctx, None, Some(&got), 1, 1)
            .expect_err("must reject");
        match err {
            CheckError::Input(message) => assert!(
                message.contains("a return statement outside a function"),
                "unexpected rejection: {message}"
            ),
            CheckError::Internal(message) => panic!("expected a subset error, got {message}"),
        }
    }

    #[test]
    fn an_empty_return_in_a_value_function_is_rejected() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let def = sig(instance("builtins.int", Vec::new()));
        let diag = check_return(&ctx, Some(&def), None, 4, 5)
            .expect("decidable")
            .expect("a diagnostic");
        let expected = "error: Return value expected  [return-value]";
        assert_eq!(diag.message, expected);
    }

    #[test]
    fn an_empty_return_in_a_none_function_is_accepted() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let def = none_sig();
        let diag = check_return(&ctx, Some(&def), None, 2, 5)
            .expect("decidable");
        assert!(diag.is_none());
    }

    #[test]
    fn an_explicit_none_returned_from_a_none_function_is_accepted() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let def = none_sig();
        let got = Type::NoneType;
        let diag = check_return(&ctx, Some(&def), Some(&got), 2, 5)
            .expect("decidable");
        assert!(diag.is_none());
    }

    #[test]
    fn a_value_returned_from_a_none_function_is_rejected() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let def = none_sig();
        let got = instance("builtins.int", Vec::new());
        let diag = check_return(&ctx, Some(&def), Some(&got), 6, 5)
            .expect("decidable")
            .expect("a diagnostic");
        assert_eq!(diag.message, "error: No return value expected  [return-value]");
    }

    #[test]
    fn a_sibling_module_rejects_instead_of_rendering() {
        let h = Harness::new();
        let ctx = h.ctx(false);
        let def = sig(instance("builtins.int", Vec::new()));
        let got = instance("builtins.str", Vec::new());
        let err = check_return(&ctx, Some(&def), Some(&got), 3, 12)
            .expect_err("must reject");
        assert!(matches!(err, CheckError::Input(_)));
    }

    // -- check_annotated_assignment ------------------------------------------

    #[test]
    fn a_matching_annotated_assignment_is_accepted() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let declared = instance("builtins.int", Vec::new());
        let facts = kernel::SimpleAssignmentFacts::default();
        let got = check_annotated_assignment(&ctx, &declared, &declared, &facts, 2, 5);
        assert!(got.expect("decidable").is_none());
    }

    #[test]
    fn an_assignment_violating_the_declared_type_renders_the_mypy_message() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let declared = instance("builtins.int", Vec::new());
        let value = instance("builtins.str", Vec::new());
        let facts = kernel::SimpleAssignmentFacts::default();
        let diag = check_annotated_assignment(&ctx, &declared, &value, &facts, 2, 5)
            .expect("decidable")
            .expect("a diagnostic");
        assert_eq!(diag.line, 2);
        assert_eq!(diag.col, 5);
        assert_eq!(
            diag.message,
            "error: Incompatible types in assignment (expression has type \"str\", \
             variable has type \"int\")  [assignment]"
        );
    }

    #[test]
    fn a_stub_ellipsis_initializer_skips_the_subtype_check() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let declared = instance("builtins.int", Vec::new());
        let value = instance("builtins.str", Vec::new());
        let facts = kernel::SimpleAssignmentFacts {
            is_stub: true,
            rvalue_is_ellipsis: true,
            ..kernel::SimpleAssignmentFacts::default()
        };
        let got = check_annotated_assignment(&ctx, &declared, &value, &facts, 2, 5);
        assert!(got.expect("decidable").is_none());
    }

    #[test]
    fn a_union_target_context_is_out_of_subset() {
        let h = Harness::new();
        let ctx = h.ctx(true);
        let declared = Type::UnionType {
            items: vec![
                instance("builtins.int", Vec::new()),
                instance("builtins.str", Vec::new()),
            ],
            uses_pep604_syntax: false,
            can_be_true: true,
            can_be_false: true,
            is_evaluated: false,
            original_str_expr: None,
            original_str_fallback: None,
        };
        let value = instance("builtins.int", Vec::new());
        let facts = kernel::SimpleAssignmentFacts {
            has_inferred: true,
            ..kernel::SimpleAssignmentFacts::default()
        };
        let err = check_annotated_assignment(&ctx, &declared, &value, &facts, 2, 5)
            .expect_err("must reject");
        assert!(matches!(err, CheckError::Input(_)));
    }

    // -- lvalue facts ---------------------------------------------------------

    #[test]
    fn a_first_binding_of_a_name_is_a_definition() {
        let facts = name_lvalue_facts(true);
        assert_eq!(kernel::check_lvalue(&facts), kernel::LvalueKind::NameDef);
    }

    #[test]
    fn a_rebinding_of_a_name_is_not_a_definition() {
        let facts = name_lvalue_facts(false);
        assert_eq!(kernel::check_lvalue(&facts), kernel::LvalueKind::Name);
    }

    #[test]
    fn a_self_attribute_target_routes_to_the_member_arm() {
        let facts = self_attr_lvalue_facts();
        assert_eq!(kernel::check_lvalue(&facts), kernel::LvalueKind::Member);
    }

    // -- augmented assignment ---------------------------------------------------

    #[test]
    fn the_inplace_table_matches_the_mypy_operator_set() {
        // mypy/operators.py:37-51, the thirteen spellings with an inplace form.
        let inplace = "+ - * / % // ** @ & | ^ << >>";
        for symbol in inplace.split(' ') {
            assert!(
                op_has_inplace_method(symbol),
                "{symbol} admits an inplace form"
            );
        }
        let plain = "< <= == != > >= and or";
        for symbol in plain.split(' ') {
            assert!(!op_has_inplace_method(symbol), "{symbol} does not");
        }
    }

    #[test]
    fn a_target_with_an_inplace_member_uses_the_inplace_method() {
        let probe = TableProbe {
            members: vec![("builtins.int", "__iadd__")],
        };
        let target = instance("builtins.int", Vec::new());
        assert_eq!(
            operator_assignment_method(BinOpKind::Add, &target, &probe),
            (true, "__iadd__".to_string())
        );
    }

    #[test]
    fn a_target_without_the_inplace_member_uses_the_forward_method() {
        let probe = TableProbe {
            members: Vec::new(),
        };
        let target = instance("builtins.str", Vec::new());
        assert_eq!(
            operator_assignment_method(BinOpKind::Add, &target, &probe),
            (false, "__add__".to_string())
        );
    }

    #[test]
    fn a_non_instance_target_never_probes_for_an_inplace_member() {
        let probe = TableProbe {
            members: vec![("builtins.int", "__iadd__")],
        };
        assert_eq!(
            operator_assignment_method(BinOpKind::Add, &Type::NoneType, &probe),
            (false, "__add__".to_string())
        );
    }

    // -- return_stmt_facts -------------------------------------------------------

    #[test]
    fn the_none_annotation_is_the_only_varying_return_fact() {
        assert!(return_stmt_facts(&Type::NoneType).declared_none_return);
        let value = instance("builtins.int", Vec::new());
        let facts = return_stmt_facts(&value);
        assert!(!facts.declared_none_return);
        assert!(facts.in_checked_function);
        assert!(!facts.is_generator);
        assert!(!facts.is_lambda);
    }
}
