//! Subset parse + lower: ruff tree in, skeleton AST out.
//!
//! The skeleton supports exactly the statement shapes the trivial corpus
//! uses (ADR-0008 Lane 2): module-level annotated assignments with literal
//! values, a fully annotated zero-argument function whose body is a
//! single literal return, and module-level `pass`. Anything
//! else is a hard error: the skeleton never guesses at semantics it does
//! not implement.

use ruff_python_ast::{self as ast, PySourceType, Stmt};
use ruff_python_parser::parse_unchecked_source;
use ruff_text_size::Ranged;

/// A literal value in the supported expression subset.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

impl Lit {
    /// The `mypy.types.Instance` fullname mypy infers for this literal.
    pub fn type_fullname(&self) -> &'static str {
        match self {
            Lit::Int(_) => "builtins.int",
            Lit::Float(_) => "builtins.float",
            Lit::Str(_) => "builtins.str",
            Lit::Bool(_) => "builtins.bool",
        }
    }
}

/// `x: <annotation> = <literal>` at module level.
#[derive(Debug)]
pub struct AnnAssignStmt {
    pub annotation: String,
    pub value: Lit,
    pub line: usize,
}

/// `def f() -> <annotation>: return <literal>`.
#[derive(Debug)]
pub struct FuncDefStmt {
    pub ret_annotation: String,
    pub ret_value: Lit,
    pub line: usize,
}

#[derive(Debug)]
pub enum TopStmt {
    AnnAssign(AnnAssignStmt),
    FuncDef(FuncDefStmt),
    Pass,
}

#[derive(Debug)]
pub struct ModuleAst {
    pub body: Vec<TopStmt>,
}

/// Parse + lower `source`. Fails hard on any construct outside the
/// subset; the message names the offending line and construct.
pub fn parse_module(source: &str, path: &str) -> Result<ModuleAst, String> {
    let parsed = parse_unchecked_source(source, PySourceType::Python);
    if let Some(err) = parsed.errors().first() {
        return Err(format!(
            "{path}:{}: skeleton subset error: syntax error: {}",
            line_of(source, err.range().start().to_usize()),
            err
        ));
    }
    let module = parsed.into_syntax();
    let mut body = Vec::with_capacity(module.body.len());
    for stmt in module.body {
        body.push(lower_top(stmt, source, path)?);
    }
    Ok(ModuleAst { body })
}

fn lower_top(stmt: Stmt, source: &str, path: &str) -> Result<TopStmt, String> {
    let line = line_of(source, stmt.range().start().to_usize());
    match stmt {
        Stmt::AnnAssign(ann) => {
            if !matches!(*ann.target, ast::Expr::Name(_)) {
                return Err(subset_error(
                    path,
                    line,
                    "assignment target must be a plain name",
                ));
            }
            let annotation = match *ann.annotation {
                ast::Expr::Name(n) => n.id.to_string(),
                _ => {
                    return Err(subset_error(
                        path,
                        line,
                        "annotation must be a bare primitive name",
                    ))
                }
            };
            let value = match ann.value {
                Some(boxed) => lower_lit(*boxed, path, line)?,
                None => {
                    return Err(subset_error(
                        path,
                        line,
                        "annotated assignment must have a literal value",
                    ))
                }
            };
            Ok(TopStmt::AnnAssign(AnnAssignStmt {
                annotation,
                value,
                line,
            }))
        }
        Stmt::FunctionDef(f) => {
            if f.is_async {
                return Err(subset_error(
                    path,
                    line,
                    "async functions are outside the skeleton subset",
                ));
            }
            if !f.decorator_list.is_empty() {
                return Err(subset_error(
                    path,
                    line,
                    "decorated functions are outside the skeleton subset",
                ));
            }
            if f.type_params.is_some() {
                return Err(subset_error(
                    path,
                    line,
                    "PEP 695 type parameters are outside the skeleton subset",
                ));
            }
            if !f.parameters.posonlyargs.is_empty()
                || !f.parameters.args.is_empty()
                || f.parameters.vararg.is_some()
                || !f.parameters.kwonlyargs.is_empty()
                || f.parameters.kwarg.is_some()
            {
                return Err(subset_error(
                    path,
                    line,
                    "function parameters are unsupported",
                ));
            }
            let ret_annotation = match f.returns {
                Some(boxed) => match *boxed {
                    ast::Expr::Name(n) => n.id.to_string(),
                    _ => {
                        return Err(subset_error(
                            path,
                            line,
                            "return annotation must be a bare primitive name",
                        ))
                    }
                },
                None => {
                    return Err(subset_error(
                        path,
                        line,
                        "function must declare a return annotation",
                    ))
                }
            };
            let ret_value = match f.body.as_slice() {
                [Stmt::Return(ret)] => match &ret.value {
                    Some(boxed) => lower_lit((**boxed).clone(), path, line)?,
                    None => {
                        return Err(subset_error(
                            path,
                            line,
                            "bare return is outside the skeleton subset: a non-None \
                             annotation requires a returned value",
                        ))
                    }
                },
                [Stmt::Pass(_)] => {
                    return Err(subset_error(
                        path,
                        line,
                        "pass body is outside the skeleton subset: a non-None \
                         annotation requires a returned value",
                    ))
                }
                _ => {
                    return Err(subset_error(
                        path,
                        line,
                        "function body must be a single literal return",
                    ))
                }
            };
            Ok(TopStmt::FuncDef(FuncDefStmt {
                ret_annotation,
                ret_value,
                line,
            }))
        }
        Stmt::Pass(_) => Ok(TopStmt::Pass),
        other => Err(subset_error(
            path,
            line,
            &format!(
                "statement `{}` is outside the skeleton subset",
                other_kind(&other)
            ),
        )),
    }
}

fn lower_lit(expr: ast::Expr, path: &str, line: usize) -> Result<Lit, String> {
    match expr {
        ast::Expr::NumberLiteral(n) => match n.value {
            ast::Number::Int(i) => match i.as_i64() {
                Some(v) => Ok(Lit::Int(v)),
                None => Err(subset_error(
                    path,
                    line,
                    "integer literal exceeds 64-bit range",
                )),
            },
            ast::Number::Float(f) => Ok(Lit::Float(f)),
            ast::Number::Complex { .. } => Err(subset_error(path, line, "complex literals are unsupported")),
        },
        ast::Expr::StringLiteral(s) => Ok(Lit::Str(s.value.to_str().to_string())),
        ast::Expr::BooleanLiteral(b) => Ok(Lit::Bool(b.value)),
        other => Err(subset_error(
            path,
            line,
            &format!(
                "expression `{}` is outside the skeleton subset (only int, float, str and bool literals are supported)",
                expr_kind(&other)
            ),
        )),
    }
}

fn other_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::ClassDef(_) => "ClassDef",
        Stmt::Import(_) => "Import",
        Stmt::ImportFrom(_) => "ImportFrom",
        Stmt::If(_) => "If",
        Stmt::While(_) => "While",
        Stmt::For(_) => "For",
        Stmt::Expr(_) => "Expr",
        Stmt::Assign(_) => "Assign",
        _ => "unsupported",
    }
}

fn expr_kind(expr: &ast::Expr) -> &'static str {
    match expr {
        ast::Expr::Name(_) => "Name",
        ast::Expr::Call(_) => "Call",
        ast::Expr::BinOp(_) => "BinOp",
        ast::Expr::Attribute(_) => "Attribute",
        _ => "unsupported",
    }
}

fn subset_error(path: &str, line: usize, detail: &str) -> String {
    format!("{path}:{line}: skeleton subset error: {detail}")
}

/// Byte offsets of line starts (0 first): the ast_serialize line map.
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn line_of(source: &str, offset: usize) -> usize {
    let starts = line_starts(source);
    starts.partition_point(|start| *start <= offset)
}
