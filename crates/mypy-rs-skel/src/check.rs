//! The checking phase of the skeleton: a micro semanal pass (annotation
//! resolution through the fixture symbol map) plus the two corpus checks
//! — assignment compatibility and function return compatibility — both
//! decided by the kernel's `is_subtype` over fixture-backed snapshots.
//!
//! The `None` verdict means the kernel itself declined to decide (a
//! missing snapshot or a live-Python consult the skeleton cannot answer).
//! For this corpus that is an internal error, not a "not a subtype"
//! verdict: fail early, never render a guess.

use type_kernel::skeleton_api::{is_subtype, SubtypeContext, Type};

use crate::fixtures::Fixtures;
use crate::subset::{ModuleAst, TopStmt};

/// A rendered diagnostic: line number plus the text after `path:line: `.
#[derive(Debug)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

/// Bind module-level symbols and check every statement, in file order.
pub fn check_module(
    ast: &ModuleAst,
    fixtures: &Fixtures,
    path: &str,
) -> Result<Vec<Diagnostic>, String> {
    let ctx = SubtypeContext {
        strict_optional: true,
        ..SubtypeContext::default()
    };
    let resolver = &fixtures.resolver;
    let mut diagnostics = Vec::new();
    for stmt in &ast.body {
        match stmt {
            TopStmt::AnnAssign(assign) => {
                let declared_name =
                    resolve_annotation(&assign.annotation, fixtures, path, assign.line)?;
                let declared = instance(&declared_name);
                let expression = instance(assign.value.type_fullname());
                let verdict = require_decidable(
                    is_subtype(&expression, &declared, &ctx, resolver),
                    "assignment",
                    path,
                    assign.line,
                )?;
                if verdict == Some(false) {
                    diagnostics.push(Diagnostic {
                        line: assign.line,
                        message: format!(
                            "error: Incompatible types in assignment (expression has type \
                             \"{expr}\", variable has type \"{var}\")  [assignment]",
                            expr = display_type(assign.value.type_fullname()),
                            var = display_type(&declared_name),
                        ),
                    });
                }
            }
            TopStmt::FuncDef(func) => {
                let declared_name =
                    resolve_annotation(&func.ret_annotation, fixtures, path, func.line)?;
                let Some(value) = &func.ret_value else {
                    continue;
                };
                let returned = instance(value.type_fullname());
                let declared_ret = instance(&declared_name);
                let verdict = require_decidable(
                    is_subtype(&returned, &declared_ret, &ctx, resolver),
                    "return",
                    path,
                    func.line,
                )?;
                if verdict == Some(false) {
                    return Err(format!(
                        "{path}:{}: skeleton subset error: return-value incompatibility is \
                         outside the supported error classes",
                        func.line
                    ));
                }
            }
            TopStmt::Pass => {}
        }
    }
    Ok(diagnostics)
}

/// `is_subtype` returns `Option<bool>`: `None` is the kernel declining to
/// decide. Every pair in the supported corpus must land a verdict; a
/// `None` here means the fixtures no longer cover the closure.
fn require_decidable(
    verdict: Option<bool>,
    check: &str,
    path: &str,
    line: usize,
) -> Result<Option<bool>, String> {
    match verdict {
        Some(v) => Ok(Some(v)),
        None => Err(format!(
            "{path}:{line}: skeleton internal error: kernel deferred the {check} check; \
             the fixture closure no longer covers the corpus"
        )),
    }
}

fn resolve_annotation(
    name: &str,
    fixtures: &Fixtures,
    path: &str,
    line: usize,
) -> Result<String, String> {
    match fixtures.builtins_symbols.get(name) {
        Some(fullname) => Ok(fullname.clone()),
        None => Err(format!(
            "{path}:{line}: skeleton subset error: annotation `{name}` is not one of the \
             primitive names the fixture symbols cover"
        )),
    }
}

/// A plain `mypy.types.Instance` of `fullname` with no arguments.
fn instance(fullname: &str) -> Type {
    Type::Instance {
        type_ref: fullname.to_string(),
        args: Vec::new(),
        last_known_value: None,
        extra_attrs: None,
    }
}

/// mypy's `TypeStrVisitor` displays builtins types without the
/// `builtins.` prefix; other fullnames display verbatim.
fn display_type(fullname: &str) -> &str {
    fullname.strip_prefix("builtins.").unwrap_or(fullname)
}
