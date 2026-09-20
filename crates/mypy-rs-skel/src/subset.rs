//! Subset parse + lower: ruff tree in, skeleton AST out.
//!
//! The skeleton supports the statement shapes of the trivial corpus plus
//! the class/member slice of #118: imports, `TypeVar` declarations, plain
//! and annotated module assignments, class definitions (single or generic
//! bases, class attributes, methods with `self` and `super()` calls),
//! module functions, calls, attribute reads, subscripts on classes and
//! the `str`/`float` binary operators the corpus uses. Anything else is a
//! hard error: the skeleton never guesses at semantics it does not
//! implement (#93 invariant, tracked as #115).

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

/// An annotation in the supported subset: a bare name, `None`, or a
/// generic subscript like `Sized[int]`. String annotations are reparsed
/// at lowering time and never reach the checker as strings.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnKind {
    Name(String),
    NoneT,
    Subscript { base: String, args: Vec<Ann> },
}

/// An annotation with the line it was written on.
#[derive(Debug, Clone, PartialEq)]
pub struct Ann {
    pub kind: AnnKind,
    pub line: usize,
}

/// A binary operator the corpus uses and the checker tabulates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOpKind {
    Add,
    Mult,
}

/// An expression in the supported subset, with its source line.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Lit(Lit),
    Name(String),
    /// The `super()` call expression, only valid as a call target inside
    /// a method body.
    Super,
    Attr {
        obj: Box<Expr>,
        name: String,
    },
    /// A subscript on a bare class name, value position: `NamedBox[int]`.
    Subscript {
        base: String,
        args: Vec<Ann>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    BinOp {
        op: BinOpKind,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

/// A class base reference: `Shape` or `Sized[T]`.
#[derive(Debug)]
pub struct BaseRef {
    pub kind: BaseRefKind,
    pub line: usize,
}

#[derive(Debug)]
pub enum BaseRefKind {
    Plain(String),
    Subscript { base: String, args: Vec<Ann> },
}

/// An annotated parameter (self is recorded separately for methods).
#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub ann: Ann,
}

/// One statement of a function or method body.
#[derive(Debug)]
pub enum BodyStmt {
    /// `self.<attr> = <value>`, methods only.
    SelfAssign {
        attr: String,
        value: Expr,
        line: usize,
    },
    /// A bare call statement, typed and discarded.
    Call(Expr),
    Return {
        value: Option<Expr>,
        line: usize,
    },
    Pass,
}

/// A statement inside a class body. `Method` reuses `FuncDefStmt`: the
/// `self` parameter is stripped at lowering and the class context is
/// carried by the variant itself.
#[derive(Debug)]
pub enum ClassStmt {
    /// `name: <ann> = <literal>`.
    VarDecl {
        name: String,
        ann: Ann,
        value: Lit,
        line: usize,
    },
    Method(FuncDefStmt),
    Pass,
}

#[derive(Debug)]
pub struct ClassDefStmt {
    pub name: String,
    pub bases: Vec<BaseRef>,
    pub body: Vec<ClassStmt>,
    pub line: usize,
}

/// A module-level function: all parameters annotated, no self.
#[derive(Debug)]
pub struct FuncDefStmt {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<Ann>,
    pub body: Vec<BodyStmt>,
    pub line: usize,
}

#[derive(Debug)]
pub enum StmtKind {
    /// `import <name>`; the name is bound as an unusable module marker.
    Import {
        names: Vec<String>,
    },
    /// `from <module> import <name>, ...`.
    ImportFrom {
        module: String,
        names: Vec<String>,
    },
    /// `<binding> = TypeVar("<tv_name>")`.
    TypeVarDecl {
        binding: String,
        tv_name: String,
    },
    /// `<name> = <value>`, no annotation.
    Assign {
        name: String,
        value: Expr,
        line: usize,
    },
    /// `<name>: <ann> = <value>`.
    AnnAssign {
        name: String,
        ann: Ann,
        value: Expr,
        line: usize,
    },
    ClassDef(ClassDefStmt),
    FuncDef(FuncDefStmt),
    Pass,
}

#[derive(Debug)]
pub struct TopStmt {
    pub kind: StmtKind,
    pub line: usize,
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

/// Parse the contents of a string annotation (`"Sized[T]"`) as one
/// expression and lower it to an `Ann`.
pub fn parse_string_annotation(source: &str, path: &str, line: usize) -> Result<Ann, String> {
    let parsed = parse_unchecked_source(source, PySourceType::Python);
    if let Some(err) = parsed.errors().first() {
        return Err(subset_error(
            path,
            line,
            &format!("string annotation is not a valid expression: {err}"),
        ));
    }
    let module = parsed.into_syntax();
    match module.body.as_slice() {
        [Stmt::Expr(expr_stmt)] => {
            let inner = &expr_stmt.value;
            match inner.as_ref() {
                ast::Expr::StringLiteral(_) => Err(subset_error(
                    path,
                    line,
                    "nested string annotations are outside the skeleton subset",
                )),
                other => lower_ann(other, path, line),
            }
        }
        _ => Err(subset_error(
            path,
            line,
            "string annotation must be a single expression",
        )),
    }
}

fn lower_top(stmt: Stmt, source: &str, path: &str) -> Result<TopStmt, String> {
    let line = line_of(source, stmt.range().start().to_usize());
    match stmt {
        Stmt::Import(imp) => {
            let mut names = Vec::with_capacity(imp.names.len());
            for alias in imp.names {
                if alias.name.id.contains('.') {
                    return Err(subset_error(
                        path,
                        line,
                        "dotted module imports are outside the skeleton subset",
                    ));
                }
                if alias.asname.is_some() {
                    return Err(subset_error(
                        path,
                        line,
                        "import aliases are outside the skeleton subset",
                    ));
                }
                names.push(alias.name.id.to_string());
            }
            Ok(TopStmt {
                kind: StmtKind::Import { names },
                line,
            })
        }
        Stmt::ImportFrom(imp) => {
            if imp.level > 0 {
                return Err(subset_error(
                    path,
                    line,
                    "relative imports are outside the skeleton subset",
                ));
            }
            let Some(module) = imp.module else {
                return Err(subset_error(
                    path,
                    line,
                    "relative imports are outside the skeleton subset",
                ));
            };
            if module.id.contains('.') {
                return Err(subset_error(
                    path,
                    line,
                    "dotted module imports are outside the skeleton subset",
                ));
            }
            let mut names = Vec::with_capacity(imp.names.len());
            for alias in imp.names {
                if alias.asname.is_some() {
                    return Err(subset_error(
                        path,
                        line,
                        "import aliases are outside the skeleton subset",
                    ));
                }
                if alias.name.id == "*" {
                    return Err(subset_error(
                        path,
                        line,
                        "star imports are outside the skeleton subset",
                    ));
                }
                if module.id == "typing" && !matches!(alias.name.id.as_str(), "Generic" | "TypeVar")
                {
                    return Err(subset_error(
                        path,
                        line,
                        &format!(
                            "`from typing import {}` is outside the skeleton subset \
                             (only Generic and TypeVar are supported)",
                            alias.name.id
                        ),
                    ));
                }
                names.push(alias.name.id.to_string());
            }
            Ok(TopStmt {
                kind: StmtKind::ImportFrom {
                    module: module.id.to_string(),
                    names,
                },
                line,
            })
        }
        Stmt::Assign(assign) => {
            if assign.targets.len() != 1 || !matches!(&assign.targets[0], ast::Expr::Name(_)) {
                return Err(subset_error(
                    path,
                    line,
                    "assignment target must be a single plain name",
                ));
            }
            let name = match &assign.targets[0] {
                ast::Expr::Name(n) => n.id.to_string(),
                _ => unreachable!("name target checked above"),
            };
            if let Some(tv_name) = type_var_decl(&assign.value, path, line)? {
                return Ok(TopStmt {
                    kind: StmtKind::TypeVarDecl {
                        binding: name,
                        tv_name,
                    },
                    line,
                });
            }
            let value = lower_expr(&assign.value, path, line)?;
            Ok(TopStmt {
                kind: StmtKind::Assign { name, value, line },
                line,
            })
        }
        Stmt::AnnAssign(ann) => {
            if !matches!(*ann.target, ast::Expr::Name(_)) {
                return Err(subset_error(
                    path,
                    line,
                    "assignment target must be a single plain name",
                ));
            }
            let name = match &*ann.target {
                ast::Expr::Name(n) => n.id.to_string(),
                _ => unreachable!("name target checked above"),
            };
            let ann_out = lower_ann(&ann.annotation, path, line)?;
            let Some(value) = ann.value else {
                return Err(subset_error(
                    path,
                    line,
                    "annotated assignment must have a value",
                ));
            };
            let value = lower_expr(&value, path, line)?;
            Ok(TopStmt {
                kind: StmtKind::AnnAssign {
                    name,
                    ann: ann_out,
                    value,
                    line,
                },
                line,
            })
        }
        Stmt::ClassDef(cls) => Ok(TopStmt {
            kind: StmtKind::ClassDef(lower_class(cls, source, path, line)?),
            line,
        }),
        Stmt::FunctionDef(f) => {
            let func = lower_function(f, source, path, line, false)?;
            Ok(TopStmt {
                kind: StmtKind::FuncDef(func),
                line,
            })
        }
        Stmt::Pass(_) => Ok(TopStmt {
            kind: StmtKind::Pass,
            line,
        }),
        other => Err(subset_error(
            path,
            line,
            &format!(
                "statement `{}` is outside the skeleton subset",
                stmt_kind_name(&other)
            ),
        )),
    }
}

/// `T = TypeVar("T")` shape: the value must be a call to the name
/// `TypeVar` with exactly one string positional argument.
fn type_var_decl(value: &ast::Expr, path: &str, line: usize) -> Result<Option<String>, String> {
    let ast::Expr::Call(call) = value else {
        return Ok(None);
    };
    if !matches!(&*call.func, ast::Expr::Name(n) if n.id == "TypeVar") {
        return Ok(None);
    }
    if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
        return Ok(None);
    }
    match &call.arguments.args[0] {
        ast::Expr::StringLiteral(s) => Ok(Some(string_lit_value(s, path, line)?)),
        _ => Ok(None),
    }
}

/// One string literal's value; implicit concatenation is rejected because
/// `to_str()` would silently keep only the first part, diverging from mypy.
fn string_lit_value(s: &ast::ExprStringLiteral, path: &str, line: usize) -> Result<String, String> {
    if s.value.is_implicit_concatenated() {
        return Err(subset_error(
            path,
            line,
            "implicitly concatenated strings are outside the skeleton subset",
        ));
    }
    Ok(s.value.to_str().to_string())
}

fn is_super_call(call: &ast::ExprCall) -> bool {
    matches!(&*call.func, ast::Expr::Name(n) if n.id == "super")
}

fn lower_class(
    cls: ast::StmtClassDef,
    source: &str,
    path: &str,
    line: usize,
) -> Result<ClassDefStmt, String> {
    if !cls.decorator_list.is_empty() {
        return Err(subset_error(
            path,
            line,
            "decorated classes are outside the skeleton subset",
        ));
    }
    if cls.type_params.is_some() {
        return Err(subset_error(
            path,
            line,
            "PEP 695 type parameters are outside the skeleton subset",
        ));
    }
    if !cls.keywords().is_empty() {
        return Err(subset_error(
            path,
            line,
            "metaclass keywords are outside the skeleton subset",
        ));
    }
    let mut bases = Vec::with_capacity(cls.bases().len());
    for base in cls.bases() {
        let base_line = line_of(source, base.range().start().to_usize());
        bases.push(lower_base_ref(base, path, base_line)?);
    }
    let mut body = Vec::with_capacity(cls.body.len());
    for stmt in cls.body {
        body.push(lower_class_stmt(stmt, source, path)?);
    }
    Ok(ClassDefStmt {
        name: cls.name.id.to_string(),
        bases,
        body,
        line,
    })
}

fn lower_base_ref(base: &ast::Expr, path: &str, line: usize) -> Result<BaseRef, String> {
    match base {
        ast::Expr::Name(n) => Ok(BaseRef {
            kind: BaseRefKind::Plain(n.id.to_string()),
            line,
        }),
        ast::Expr::Subscript(sub) => {
            let Some(base_name) = base_head_name(&sub.value) else {
                return Err(subset_error(
                    path,
                    line,
                    "subscript base must be a bare class name",
                ));
            };
            let args = subscript_args(&sub.slice, path, line)?;
            Ok(BaseRef {
                kind: BaseRefKind::Subscript {
                    base: base_name,
                    args,
                },
                line,
            })
        }
        _ => Err(subset_error(
            path,
            line,
            "class bases must be names or generic subscripts",
        )),
    }
}

fn base_head_name(value: &ast::Expr) -> Option<String> {
    match value {
        ast::Expr::Name(n) => Some(n.id.to_string()),
        _ => None,
    }
}

/// Subscript arguments: a tuple slice yields several, anything else one.
fn subscript_args(slice: &ast::Expr, path: &str, line: usize) -> Result<Vec<Ann>, String> {
    match slice {
        ast::Expr::Tuple(tuple) => {
            let mut args = Vec::with_capacity(tuple.elts.len());
            for item in &tuple.elts {
                args.push(lower_ann(item, path, line)?);
            }
            Ok(args)
        }
        single => Ok(vec![lower_ann(single, path, line)?]),
    }
}

fn lower_class_stmt(stmt: Stmt, source: &str, path: &str) -> Result<ClassStmt, String> {
    let line = line_of(source, stmt.range().start().to_usize());
    match stmt {
        Stmt::AnnAssign(ann) => {
            if !matches!(*ann.target, ast::Expr::Name(_)) {
                return Err(subset_error(
                    path,
                    line,
                    "class attribute target must be a single plain name",
                ));
            }
            let name = match &*ann.target {
                ast::Expr::Name(n) => n.id.to_string(),
                _ => unreachable!("name target checked above"),
            };
            let ann_out = lower_ann(&ann.annotation, path, line)?;
            let Some(value) = ann.value else {
                return Err(subset_error(
                    path,
                    line,
                    "class attribute declarations without a value are outside the skeleton subset",
                ));
            };
            let value = lower_lit(&value, path, line)?;
            Ok(ClassStmt::VarDecl {
                name,
                ann: ann_out,
                value,
                line,
            })
        }
        Stmt::FunctionDef(f) => {
            let method = lower_function(f, source, path, line, true)?;
            Ok(ClassStmt::Method(method))
        }
        Stmt::Pass(_) => Ok(ClassStmt::Pass),
        other => Err(subset_error(
            path,
            line,
            &format!(
                "statement `{}` is outside the skeleton subset in a class body",
                stmt_kind_name(&other)
            ),
        )),
    }
}

/// Shared lowering for module functions and methods. A method must have
/// exactly one un-annotated `self` parameter first; every other parameter
/// is annotated and carries no default.
fn lower_function(
    f: ast::StmtFunctionDef,
    source: &str,
    path: &str,
    line: usize,
    is_method: bool,
) -> Result<FuncDefStmt, String> {
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
        || f.parameters.vararg.is_some()
        || !f.parameters.kwonlyargs.is_empty()
        || f.parameters.kwarg.is_some()
    {
        return Err(subset_error(
            path,
            line,
            "positional-only, variadic and keyword-only parameters are outside \
             the skeleton subset",
        ));
    }
    let mut raw_params: Vec<(&ast::Parameter, usize)> = Vec::new();
    for param in f.parameters.args.iter() {
        let param_line = line_of(source, param.range().start().to_usize());
        if param.default.is_some() {
            return Err(subset_error(
                path,
                param_line,
                "default parameter values are outside the skeleton subset",
            ));
        }
        raw_params.push((&param.parameter, param_line));
    }
    let mut params = Vec::with_capacity(raw_params.len());
    if is_method {
        let Some((first, first_line)) = raw_params.first() else {
            return Err(subset_error(
                path,
                line,
                "methods must declare a self parameter",
            ));
        };
        if first.name.id != "self" {
            return Err(subset_error(
                path,
                *first_line,
                "the first method parameter must be named self",
            ));
        }
        if first.annotation.is_some() {
            return Err(subset_error(
                path,
                *first_line,
                "an annotated self parameter is outside the skeleton subset",
            ));
        }
        raw_params.remove(0);
    }
    for (param, param_line) in raw_params {
        let Some(annotation) = param.annotation.as_deref() else {
            return Err(subset_error(
                path,
                param_line,
                "parameters must be annotated",
            ));
        };
        params.push(Param {
            name: param.name.id.to_string(),
            ann: lower_ann(annotation, path, param_line)?,
        });
    }
    let ret = match f.returns.as_deref() {
        Some(annotation) => {
            let ret_line = line_of(source, annotation.range().start().to_usize());
            Some(lower_ann(annotation, path, ret_line)?)
        }
        None => {
            return Err(subset_error(
                path,
                line,
                "function must declare a return annotation",
            ))
        }
    };
    let mut body = Vec::with_capacity(f.body.len());
    for stmt in f.body {
        body.push(lower_body_stmt(stmt, source, path, is_method)?);
    }
    check_body_shape(&body, &ret, path, line)?;
    Ok(FuncDefStmt {
        name: f.name.id.to_string(),
        params,
        ret,
        body,
        line,
    })
}

/// Body shape rules per return annotation: a `None` return forbids any
/// returned value; any other return requires the body to end in a return
/// with a value, and no earlier return at all.
fn check_body_shape(
    body: &[BodyStmt],
    ret: &Option<Ann>,
    path: &str,
    line: usize,
) -> Result<(), String> {
    let none_ret = matches!(ret, Some(ann) if ann.kind == AnnKind::NoneT);
    for (index, stmt) in body.iter().enumerate() {
        let is_last = index + 1 == body.len();
        if let BodyStmt::Return { value, line: at } = stmt {
            match (none_ret, value) {
                (true, Some(_)) => {
                    return Err(subset_error(
                        path,
                        *at,
                        "a returned value in a None-annotated function is outside \
                         the skeleton subset",
                    ))
                }
                (true, None) => {
                    if !is_last {
                        return Err(subset_error(
                            path,
                            *at,
                            "a return before the final statement of a None-annotated \
                             function is outside the skeleton subset",
                        ));
                    }
                }
                (false, Some(_)) => {
                    if !is_last {
                        return Err(subset_error(
                            path,
                            *at,
                            "a return before the final statement is outside \
                             the skeleton subset",
                        ));
                    }
                }
                (false, None) => {
                    return Err(subset_error(
                        path,
                        *at,
                        "bare return is outside the skeleton subset: a non-None \
                         annotation requires a returned value",
                    ))
                }
            }
        }
    }
    if !none_ret {
        match body.last() {
            Some(BodyStmt::Return { value: Some(_), .. }) => {}
            Some(BodyStmt::Pass) | Some(BodyStmt::Return { value: None, .. }) => {
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
                    "function body must end in a returned value",
                ))
            }
        }
    }
    Ok(())
}

fn lower_body_stmt(
    stmt: Stmt,
    source: &str,
    path: &str,
    is_method: bool,
) -> Result<BodyStmt, String> {
    let line = line_of(source, stmt.range().start().to_usize());
    match stmt {
        Stmt::Return(ret) => Ok(BodyStmt::Return {
            value: ret
                .value
                .map(|boxed| lower_expr(&boxed, path, line))
                .transpose()?,
            line,
        }),
        Stmt::Expr(expr_stmt) => {
            if let ast::Expr::Call(_) = &*expr_stmt.value {
                Ok(BodyStmt::Call(lower_expr(&expr_stmt.value, path, line)?))
            } else {
                Err(subset_error(
                    path,
                    line,
                    "expression statements in a body must be calls",
                ))
            }
        }
        Stmt::Assign(assign) => {
            if is_method && assign.targets.len() == 1 {
                if let ast::Expr::Attribute(attr) = &assign.targets[0] {
                    if let ast::Expr::Name(obj) = &*attr.value {
                        if obj.id == "self" {
                            let value = lower_expr(&assign.value, path, line)?;
                            return Ok(BodyStmt::SelfAssign {
                                attr: attr.attr.id.to_string(),
                                value,
                                line,
                            });
                        }
                    }
                }
            }
            Err(subset_error(
                path,
                line,
                "local assignments in a body are outside the skeleton subset",
            ))
        }
        Stmt::Pass(_) => Ok(BodyStmt::Pass),
        other => Err(subset_error(
            path,
            line,
            &format!(
                "statement `{}` is outside the skeleton subset in a body",
                stmt_kind_name(&other)
            ),
        )),
    }
}

fn lower_ann(expr: &ast::Expr, path: &str, line: usize) -> Result<Ann, String> {
    let kind = match expr {
        ast::Expr::Name(n) => AnnKind::Name(n.id.to_string()),
        ast::Expr::NoneLiteral(_) => AnnKind::NoneT,
        ast::Expr::StringLiteral(s) => {
            return parse_string_annotation(&string_lit_value(s, path, line)?, path, line)
        }
        ast::Expr::Subscript(sub) => {
            let Some(base) = base_head_name(&sub.value) else {
                return Err(subset_error(
                    path,
                    line,
                    "subscript base must be a bare class name",
                ));
            };
            AnnKind::Subscript {
                base,
                args: subscript_args(&sub.slice, path, line)?,
            }
        }
        other => {
            return Err(subset_error(
                path,
                line,
                &format!(
                    "annotation `{}` is outside the skeleton subset",
                    expr_kind_name(other)
                ),
            ))
        }
    };
    Ok(Ann { kind, line })
}

fn lower_expr(expr: &ast::Expr, path: &str, line: usize) -> Result<Expr, String> {
    let kind = match expr {
        ast::Expr::NumberLiteral(n) => ExprKind::Lit(lower_number(n, path, line)?),
        ast::Expr::StringLiteral(s) => ExprKind::Lit(Lit::Str(string_lit_value(s, path, line)?)),
        ast::Expr::BooleanLiteral(b) => ExprKind::Lit(Lit::Bool(b.value)),
        ast::Expr::Name(n) => ExprKind::Name(n.id.to_string()),
        ast::Expr::NoneLiteral(_) => {
            return Err(subset_error(
                path,
                line,
                "None literals in value position are outside the skeleton subset",
            ))
        }
        ast::Expr::Attribute(attr) => {
            let obj = match attr.value.as_ref() {
                inner @ ast::Expr::Name(_) => lower_expr(inner, path, line)?,
                ast::Expr::Call(call) if is_super_call(call) => {
                    lower_expr(&attr.value, path, line)?
                }
                _ => {
                    return Err(subset_error(
                        path,
                        line,
                        "attribute access is only supported on names and super()",
                    ))
                }
            };
            ExprKind::Attr {
                obj: Box::new(obj),
                name: attr.attr.id.to_string(),
            }
        }
        ast::Expr::Subscript(sub) => {
            let Some(base) = base_head_name(&sub.value) else {
                return Err(subset_error(
                    path,
                    line,
                    "subscript base must be a bare class name",
                ));
            };
            ExprKind::Subscript {
                base,
                args: subscript_args(&sub.slice, path, line)?,
            }
        }
        ast::Expr::Call(call) => {
            if matches!(&*call.func, ast::Expr::Name(n) if n.id == "super") {
                if !call.arguments.keywords.is_empty() {
                    return Err(subset_error(
                        path,
                        line,
                        "keyword arguments are outside the skeleton subset",
                    ));
                }
                if !call.arguments.args.is_empty() {
                    return Err(subset_error(
                        path,
                        line,
                        "super() with arguments is outside the skeleton subset",
                    ));
                }
                ExprKind::Super
            } else {
                if !call.arguments.keywords.is_empty() {
                    return Err(subset_error(
                        path,
                        line,
                        "keyword arguments are outside the skeleton subset",
                    ));
                }
                let func = lower_expr(&call.func, path, line)?;
                let mut args = Vec::with_capacity(call.arguments.args.len());
                for arg in &call.arguments.args {
                    args.push(lower_expr(arg, path, line)?);
                }
                ExprKind::Call {
                    func: Box::new(func),
                    args,
                }
            }
        }
        ast::Expr::BinOp(binop) => {
            let op = match binop.op {
                ast::Operator::Add => BinOpKind::Add,
                ast::Operator::Mult => BinOpKind::Mult,
                other => {
                    return Err(subset_error(
                        path,
                        line,
                        &format!(
                            "binary operator `{}` is outside the skeleton subset",
                            operator_name(other)
                        ),
                    ))
                }
            };
            ExprKind::BinOp {
                op,
                left: Box::new(lower_expr(&binop.left, path, line)?),
                right: Box::new(lower_expr(&binop.right, path, line)?),
            }
        }
        other => {
            return Err(subset_error(
                path,
                line,
                &format!(
                    "expression `{}` is outside the skeleton subset",
                    expr_kind_name(other)
                ),
            ))
        }
    };
    Ok(Expr { kind, line })
}

fn lower_number(n: &ast::ExprNumberLiteral, path: &str, line: usize) -> Result<Lit, String> {
    match &n.value {
        ast::Number::Int(i) => match i.as_i64() {
            Some(v) => Ok(Lit::Int(v)),
            None => Err(subset_error(
                path,
                line,
                "integer literal exceeds 64-bit range",
            )),
        },
        ast::Number::Float(f) => Ok(Lit::Float(*f)),
        ast::Number::Complex { .. } => Err(subset_error(
            path,
            line,
            "complex literals are outside the skeleton subset",
        )),
    }
}

fn lower_lit(expr: &ast::Expr, path: &str, line: usize) -> Result<Lit, String> {
    match lower_expr(expr, path, line)? {
        Expr {
            kind: ExprKind::Lit(lit),
            ..
        } => Ok(lit),
        _ => Err(subset_error(
            path,
            line,
            "class attribute values must be literals",
        )),
    }
}

fn stmt_kind_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::ClassDef(_) => "ClassDef",
        Stmt::Import(_) => "Import",
        Stmt::ImportFrom(_) => "ImportFrom",
        Stmt::If(_) => "If",
        Stmt::While(_) => "While",
        Stmt::For(_) => "For",
        Stmt::Expr(_) => "Expr",
        Stmt::Assign(_) => "Assign",
        Stmt::AugAssign(_) => "AugAssign",
        Stmt::Delete(_) => "Delete",
        Stmt::With(_) => "With",
        Stmt::Try(_) => "Try",
        Stmt::Raise(_) => "Raise",
        Stmt::Assert(_) => "Assert",
        _ => "unsupported",
    }
}

fn expr_kind_name(expr: &ast::Expr) -> &'static str {
    match expr {
        ast::Expr::Name(_) => "Name",
        ast::Expr::Call(_) => "Call",
        ast::Expr::BinOp(_) => "BinOp",
        ast::Expr::Attribute(_) => "Attribute",
        ast::Expr::Subscript(_) => "Subscript",
        ast::Expr::Lambda(_) => "Lambda",
        ast::Expr::If(_) => "IfExp",
        ast::Expr::Tuple(_) => "Tuple",
        ast::Expr::List(_) => "List",
        ast::Expr::Dict(_) => "Dict",
        ast::Expr::FString(_) => "FString",
        _ => "unsupported",
    }
}

fn operator_name(op: ast::Operator) -> &'static str {
    match op {
        ast::Operator::Add => "+",
        ast::Operator::Sub => "-",
        ast::Operator::Mult => "*",
        ast::Operator::MatMult => "@",
        ast::Operator::Div => "/",
        ast::Operator::Mod => "%",
        ast::Operator::Pow => "**",
        ast::Operator::LShift => "<<",
        ast::Operator::RShift => ">>",
        ast::Operator::BitOr => "|",
        ast::Operator::BitXor => "^",
        ast::Operator::BitAnd => "&",
        ast::Operator::FloorDiv => "//",
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
