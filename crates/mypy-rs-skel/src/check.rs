//! The checking phase: a micro semanal pass over the skeleton AST that
//! builds class models, resolves annotations and members through kernel
//! snapshots, and decides every compatibility question with the
//! kernel's `is_subtype`. Supported results are the rendered assignment
//! diagnostic and Success; anything the slice does not model is a hard
//! out-of-subset error, never a silent divergence (#93, tracked #115).

use std::collections::{HashMap, HashSet};

use type_kernel::skeleton_api::{is_subtype, SubtypeContext, Type, TypeResolver};

use crate::fixtures::Fixtures;
use crate::model::{
    self, frame_env, instance, object_type, subst, subst_sig, ClassModel, Member, Sig, GENERIC_MRO,
    OBJECT_MRO,
};
use crate::subset::{
    always_returns, Ann, AnnKind, BaseRefKind, BinOpKind, BodyStmt, BoolOpKind, ClassStmt,
    CmpOpKind, Expr, ExprKind, FuncDefStmt, StmtKind,
};

/// A rendered diagnostic: line number plus the text after `path:line: `.
#[derive(Debug)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

/// A check-phase failure split by whose contract broke: `Input` means the
/// file left the supported subset (driver exit 2), `Internal` means the
/// fixtures or kernel failed on a covered file (driver exit 3).
#[derive(Debug)]
pub enum CheckError {
    Input(String),
    Internal(String),
}

/// `is_subtype` returns `Option<bool>`: `None` is the kernel declining
/// to decide. Every pair in the supported corpus must land a verdict; a
/// `None` here means the fixtures no longer cover the closure.
fn require_decidable(
    verdict: Option<bool>,
    check: &str,
    path: &str,
    line: usize,
) -> Result<bool, CheckError> {
    match verdict {
        Some(v) => Ok(v),
        None => Err(CheckError::Internal(format!(
            "{path}:{line}: skeleton internal error: kernel deferred the {check} check; \
             the fixture closure no longer covers the corpus"
        ))),
    }
}

/// mypy's `TypeStrVisitor` displays builtins types without the
/// `builtins.` prefix; other fullnames display verbatim.
fn display_type(fullname: &str) -> &str {
    fullname.strip_prefix("builtins.").unwrap_or(fullname)
}

/// The comparison operand table: the numeric primitives mypy compares
/// with a bool result.
fn is_comparison_operand(fullname: &str) -> bool {
    matches!(
        fullname,
        "builtins.int" | "builtins.float" | "builtins.bool"
    )
}

fn input(path: &str, line: usize, detail: &str) -> CheckError {
    CheckError::Input(format!("{path}:{line}: skeleton subset error: {detail}"))
}

/// One module-level binding of the micro semanal pass.
#[derive(Clone)]
enum Binding {
    Var(Type),
    Class(String),
    Function(Sig),
    /// `<binding> = TypeVar(...)`: `name` is the string passed to
    /// `TypeVar`, `fullname` the module-level binding's fullname.
    TypeVar {
        name: String,
        fullname: String,
    },
    /// An `import <name>` binding: the name itself is the module marker.
    Module,
    /// A `from typing import ...` name: only `Generic[...]` in class
    /// bases reads it; every other use is out of subset.
    Marker,
}

/// The class frame a method is checked in: the class fullname plus the
/// type variables bound by its generic bases (mypy's
/// `TypeVarLikeScope.class_frame`).
struct ClassFrame {
    fullname: String,
    tvars: Vec<model::TvarInfo>,
}

/// `resolve_bases` output: the resolved (base fullname, base args)
/// pairs plus each base's MRO, used by the linearizer.
type ResolvedBases = (Vec<(String, Vec<Type>)>, Vec<Vec<String>>);

/// The per-class state one collection sweep walks with: the lookups
/// a candidate's value typing needs, shared by every method body.
struct CollectCtx<'a> {
    bindings: &'a HashMap<String, Binding>,
    frame: &'a ClassFrame,
    self_ty: Type,
    model: &'a ClassModel,
}

/// The name-resolution environment of one checked position.
struct Scope<'a> {
    module: &'a HashMap<String, Binding>,
    /// Function/method locals; `None` at module level.
    locals: Option<&'a HashMap<String, Type>>,
    frame: Option<&'a ClassFrame>,
}

/// A member lookup result, already substituted for the receiver's type
/// arguments.
enum Found {
    Method(Sig),
    Var(Type),
}

/// The checking driver: owns the resolver, the class registry and the
/// cross-module state, and walks every file top to bottom.
pub struct Driver {
    ctx: SubtypeContext,
    resolver: TypeResolver,
    builtins: HashMap<String, String>,
    classes: HashMap<String, ClassModel>,
    modules: HashMap<String, HashMap<String, Binding>>,
    /// Modules currently being checked: the import-cycle guard.
    checking: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
    is_main: bool,
    path: String,
}

impl Driver {
    pub fn new(fixtures: Fixtures) -> Self {
        let Fixtures {
            resolver,
            builtins_symbols,
        } = fixtures;
        Driver {
            ctx: SubtypeContext {
                strict_optional: true,
                ..SubtypeContext::default()
            },
            resolver,
            builtins: builtins_symbols,
            classes: HashMap::new(),
            modules: HashMap::new(),
            checking: HashSet::new(),
            diagnostics: Vec::new(),
            is_main: false,
            path: String::new(),
        }
    }

    /// Check `path` as the driver's main file and return its diagnostics.
    /// Imported siblings are checked first; only the main file renders.
    pub fn check_main(
        &mut self,
        path: &str,
        module: &str,
        dir: &str,
        source: &str,
    ) -> Result<Vec<Diagnostic>, CheckError> {
        self.check_module_file(path, module, dir, source, true)?;
        Ok(std::mem::take(&mut self.diagnostics))
    }

    fn check_module_file(
        &mut self,
        path: &str,
        module: &str,
        dir: &str,
        source: &str,
        is_main: bool,
    ) -> Result<(), CheckError> {
        if self.modules.contains_key(module) {
            return Ok(());
        }
        if !self.checking.insert(module.to_string()) {
            return Err(CheckError::Internal(format!(
                "the cycle guard for `{module}` fired outside a checked import"
            )));
        }
        let saved_path = std::mem::replace(&mut self.path, path.to_string());
        let saved_main = std::mem::replace(&mut self.is_main, is_main);
        let result = self.check_module_inner(path, module, dir, source);
        self.path = saved_path;
        self.is_main = saved_main;
        self.checking.remove(module);
        match result {
            Ok(bindings) => {
                self.modules.insert(module.to_string(), bindings);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn check_module_inner(
        &mut self,
        path: &str,
        module: &str,
        dir: &str,
        source: &str,
    ) -> Result<HashMap<String, Binding>, CheckError> {
        let ast = crate::subset::parse_module(source, path).map_err(CheckError::Input)?;
        let mut bindings: HashMap<String, Binding> = HashMap::new();
        // Three passes, mirroring mypy's bind-then-check split (#126):
        // names bind in statement order, bodies check only after every
        // name of the module is bound; module assignments stay eager.
        for stmt in &ast.body {
            self.bind_top_stmt(&mut bindings, module, dir, &stmt.kind, stmt.line)?;
        }
        // Retry the classes' pending instance attributes against the
        // full module bindings, then check the deferred bodies.
        self.collect_instance_vars(&bindings, module, &ast.body)?;
        for stmt in &ast.body {
            self.check_top_bodies(&bindings, module, &stmt.kind)?;
        }
        Ok(bindings)
    }

    /// Load and check a sibling module of the importing file.
    fn load_sibling(&mut self, dir: &str, dep: &str, line: usize) -> Result<(), CheckError> {
        if self.modules.contains_key(dep) {
            return Ok(());
        }
        if self.checking.contains(dep) {
            return Err(input(
                &self.path,
                line,
                &format!("an import cycle through `{dep}` is outside the skeleton subset"),
            ));
        }
        let dep_path = if dir.is_empty() {
            format!("{dep}.py")
        } else {
            format!("{dir}/{dep}.py")
        };
        let source = std::fs::read_to_string(&dep_path).map_err(|e| {
            input(
                &self.path,
                line,
                &format!("cannot read the imported module `{dep}`: {e}"),
            )
        })?;
        self.check_module_file(&dep_path, dep, dir, &source, false)
    }

    /// mypy reports a name-redefinition error; the subset rejects the
    /// rebind instead of modeling the rebinding rules.
    fn reject_module_rebind(
        &self,
        bindings: &HashMap<String, Binding>,
        name: &str,
        line: usize,
    ) -> Result<(), CheckError> {
        if bindings.contains_key(name) {
            return Err(input(
                &self.path,
                line,
                "rebinding a module-level name is outside the skeleton subset",
            ));
        }
        Ok(())
    }

    /// Pass 1: bind one top-level statement's names. Function and
    /// class bodies are deferred to `check_top_bodies`.
    fn bind_top_stmt(
        &mut self,
        bindings: &mut HashMap<String, Binding>,
        module: &str,
        dir: &str,
        kind: &StmtKind,
        line: usize,
    ) -> Result<(), CheckError> {
        match kind {
            StmtKind::Import { names } => {
                for name in names {
                    self.reject_module_rebind(bindings, name, line)?;
                }
                for name in names {
                    self.load_sibling(dir, name, line)?;
                    bindings.insert(name.clone(), Binding::Module);
                }
            }
            StmtKind::ImportFrom { module: dep, names } => {
                for name in names {
                    self.reject_module_rebind(bindings, name, line)?;
                }
                if dep == "typing" {
                    for name in names {
                        bindings.insert(name.clone(), Binding::Marker);
                    }
                    return Ok(());
                }
                self.load_sibling(dir, dep, line)?;
                let dep_bindings = self.modules.get(dep).ok_or_else(|| {
                    CheckError::Internal(format!("module {dep} vanished after checking"))
                })?;
                for name in names {
                    let binding = dep_bindings.get(name).ok_or_else(|| {
                        input(
                            &self.path,
                            line,
                            &format!("`{name}` is not defined in module {dep}"),
                        )
                    })?;
                    let rebound = match binding {
                        Binding::Class(full) => Binding::Class(full.clone()),
                        Binding::Function(sig) => Binding::Function(sig.clone()),
                        Binding::Var(t) => Binding::Var(t.clone()),
                        Binding::TypeVar { .. } | Binding::Module | Binding::Marker => {
                            return Err(input(
                                &self.path,
                                line,
                                &format!(
                                    "importing `{name}` from {dep} is outside the skeleton subset"
                                ),
                            ))
                        }
                    };
                    bindings.insert(name.clone(), rebound);
                }
            }
            StmtKind::TypeVarDecl { binding, tv_name } => {
                if !matches!(bindings.get("TypeVar"), Some(Binding::Marker)) {
                    return Err(input(
                        &self.path,
                        line,
                        "TypeVar must be imported from typing before it can be called",
                    ));
                }
                if binding != tv_name {
                    return Err(input(
                        &self.path,
                        line,
                        "a TypeVar string that differs from its variable name is \
                         outside the skeleton subset",
                    ));
                }
                self.reject_module_rebind(bindings, binding, line)?;
                bindings.insert(
                    binding.clone(),
                    Binding::TypeVar {
                        name: tv_name.clone(),
                        fullname: format!("{module}.{binding}"),
                    },
                );
            }
            StmtKind::Assign { name, value, line } => {
                self.reject_module_rebind(bindings, name, *line)?;
                let scope = Scope {
                    module: bindings,
                    locals: None,
                    frame: None,
                };
                let t = self.type_expr(&scope, value)?;
                bindings.insert(name.clone(), Binding::Var(t));
            }
            StmtKind::AnnAssign {
                name,
                ann,
                value,
                line,
            } => {
                self.reject_module_rebind(bindings, name, *line)?;
                let scope = Scope {
                    module: bindings,
                    locals: None,
                    frame: None,
                };
                let declared = self.resolve_ann(&scope, ann)?;
                let expr_t = self.type_expr(&scope, value)?;
                let verdict = require_decidable(
                    self.sub(&expr_t, &declared),
                    "assignment",
                    &self.path,
                    *line,
                )?;
                if !verdict {
                    self.incompatible_assignment(&expr_t, &declared, *line)?;
                }
                bindings.insert(name.clone(), Binding::Var(declared));
            }
            StmtKind::ClassDef(cls) => self.process_class(bindings, module, cls)?,
            StmtKind::FuncDef(func) => self.bind_function(bindings, func)?,
            StmtKind::Pass => {}
        }
        Ok(())
    }

    /// An AnnAssign whose expression is not a subtype of the declared
    /// variable: the one rendered error class, and only when both sides
    /// are builtins instances (the fixture-covered primitives); every
    /// other incompatibility is out of subset.
    fn incompatible_assignment(
        &mut self,
        expr_t: &Type,
        declared: &Type,
        line: usize,
    ) -> Result<(), CheckError> {
        if self.is_main {
            if let (Type::Instance { type_ref: e, .. }, Type::Instance { type_ref: v, .. }) =
                (expr_t, declared)
            {
                if e.starts_with("builtins.") && v.starts_with("builtins.") {
                    self.diagnostics.push(Diagnostic {
                        line,
                        message: format!(
                            "error: Incompatible types in assignment (expression has type \
                             \"{expr}\", variable has type \"{var}\")  [assignment]",
                            expr = display_type(e),
                            var = display_type(v),
                        ),
                    });
                    return Ok(());
                }
            }
        }
        Err(input(
            &self.path,
            line,
            "assignment incompatibility is outside the supported error classes",
        ))
    }

    /// Insert or replace a class's snapshot in the resolver so member
    /// consults through the kernel see the current model.
    fn refresh(&mut self, fullname: &str) -> Result<(), CheckError> {
        let model_ref = self.classes.get(fullname).ok_or_else(|| {
            CheckError::Internal(format!("the class model for {fullname} is missing"))
        })?;
        model::refresh_snapshot(model_ref, &mut self.resolver).map_err(CheckError::Internal)
    }

    fn register_member(
        &mut self,
        class_fullname: &str,
        name: &str,
        member: Member,
        line: usize,
    ) -> Result<(), CheckError> {
        let model_ref = self.classes.get_mut(class_fullname).ok_or_else(|| {
            CheckError::Internal(format!("the class model for {class_fullname} is missing"))
        })?;
        if model_ref.members.contains_key(name) {
            return Err(input(
                &self.path,
                line,
                "rebinding a class member is outside the skeleton subset",
            ));
        }
        model_ref.members.insert(name.to_string(), member);
        Ok(())
    }

    fn process_class(
        &mut self,
        bindings: &mut HashMap<String, Binding>,
        module: &str,
        cls: &crate::subset::ClassDefStmt,
    ) -> Result<(), CheckError> {
        let fullname = format!("{module}.{}", cls.name);
        if self.classes.contains_key(&fullname) || bindings.contains_key(&cls.name) {
            return Err(input(
                &self.path,
                cls.line,
                "rebinding the class name is outside the skeleton subset",
            ));
        }
        let frame = self.bind_class_frame(bindings, module, cls)?;
        let (bases, base_mros) = self.resolve_bases(bindings, &frame, cls)?;
        let mro =
            model::linearize(&fullname, &base_mros).map_err(|e| input(&self.path, cls.line, &e))?;
        let model_ref = ClassModel {
            fullname: fullname.clone(),
            tvars: frame.tvars.clone(),
            bases,
            mro,
            members: std::collections::BTreeMap::new(),
        };
        self.classes.insert(fullname.clone(), model_ref);
        self.refresh(&fullname)?;
        // Bind before the body: methods construct their own class.
        bindings.insert(cls.name.clone(), Binding::Class(fullname.clone()));
        for stmt in &cls.body {
            match stmt {
                ClassStmt::VarDecl {
                    name,
                    ann,
                    value,
                    line,
                } => {
                    let scope = Scope {
                        module: bindings,
                        locals: None,
                        frame: Some(&frame),
                    };
                    let declared = self.resolve_ann(&scope, ann)?;
                    let vt = instance(value.type_fullname(), Vec::new());
                    let verdict = require_decidable(
                        self.sub(&vt, &declared),
                        "class-level assignment",
                        &self.path,
                        *line,
                    )?;
                    if !verdict {
                        return Err(input(
                            &self.path,
                            *line,
                            "class-level assignment incompatibility is outside \
                             the supported error classes",
                        ));
                    }
                    let receiver = {
                        let model_ref = self.classes.get(&fullname).ok_or_else(|| {
                            CheckError::Internal(format!(
                                "the class model for {fullname} is missing"
                            ))
                        })?;
                        instance(&fullname, model::class_frame_tvars(model_ref))
                    };
                    if self
                        .find_member(&receiver, name, Some(&fullname))?
                        .is_some()
                    {
                        return Err(input(
                            &self.path,
                            *line,
                            "overriding a base class member is outside the skeleton subset",
                        ));
                    }
                    self.register_member(&fullname, name, Member::ClassVar(declared), *line)?;
                }
                ClassStmt::Method(func) => self.bind_method(bindings, &frame, &fullname, func)?,
                ClassStmt::Pass => {}
            }
        }
        // Splice the pass-1 members (class variables, method
        // signatures) before the sweep: its body reads must resolve
        // the class's own override, not a same-named base member.
        self.refresh(&fullname)?;
        // Statement-order collection (#126): module statements after
        // this class may read its instance attributes, so they must be
        // registered before the next statement types.
        loop {
            if !self.collect_class_sweep_once(bindings, cls, &fullname)? {
                break;
            }
        }
        Ok(())
    }

    /// Bind the class frame's type variables: every base-subscript name
    /// that resolves to a module TypeVar binding, in base order.
    fn bind_class_frame(
        &mut self,
        bindings: &HashMap<String, Binding>,
        module: &str,
        cls: &crate::subset::ClassDefStmt,
    ) -> Result<ClassFrame, CheckError> {
        let mut frame = ClassFrame {
            fullname: format!("{module}.{}", cls.name),
            tvars: Vec::new(),
        };
        for base in &cls.bases {
            if let BaseRefKind::Subscript { args, .. } = &base.kind {
                for arg in args {
                    let AnnKind::Name(name) = &arg.kind else {
                        continue;
                    };
                    let Some(Binding::TypeVar {
                        name: tv_name,
                        fullname: tv_full,
                    }) = bindings.get(name)
                    else {
                        continue;
                    };
                    if frame.tvars.iter().any(|t| t.name == *tv_name) {
                        return Err(input(
                            &self.path,
                            cls.line,
                            "a type variable appearing twice in class bases is outside \
                             the skeleton subset",
                        ));
                    }
                    frame.tvars.push(model::TvarInfo {
                        name: tv_name.clone(),
                        binding: name.clone(),
                        fullname: tv_full.clone(),
                        raw_id: frame.tvars.len() as i64 + 1,
                    });
                }
            }
        }
        Ok(frame)
    }

    fn resolve_bases(
        &self,
        bindings: &HashMap<String, Binding>,
        frame: &ClassFrame,
        cls: &crate::subset::ClassDefStmt,
    ) -> Result<ResolvedBases, CheckError> {
        let scope = Scope {
            module: bindings,
            locals: None,
            frame: Some(frame),
        };
        let mut bases: Vec<(String, Vec<Type>)> = Vec::new();
        let mut base_mros: Vec<Vec<String>> = Vec::new();
        for base in &cls.bases {
            match &base.kind {
                BaseRefKind::Plain(name) => match bindings.get(name) {
                    Some(Binding::Marker) => {
                        return Err(input(
                            &self.path,
                            base.line,
                            &format!("a bare `{name}` base is outside the skeleton subset"),
                        ))
                    }
                    Some(Binding::Class(full)) => {
                        let base_model = self.base_model(name, full, base.line)?;
                        if !base_model.tvars.is_empty() {
                            return Err(input(
                                &self.path,
                                base.line,
                                &format!("the generic base `{name}` must be subscripted"),
                            ));
                        }
                        base_mros.push(base_model.mro.clone());
                        bases.push((full.clone(), Vec::new()));
                    }
                    _ => {
                        return Err(input(
                            &self.path,
                            base.line,
                            &format!("the base `{name}` is not a class in the subset"),
                        ))
                    }
                },
                BaseRefKind::Subscript { base: head, args } => match bindings.get(head) {
                    Some(Binding::Marker) => {
                        if head != "Generic" {
                            return Err(input(
                                &self.path,
                                base.line,
                                &format!(
                                    "`{head}[...]` as a base class is outside the skeleton subset"
                                ),
                            ));
                        }
                        let mut resolved = Vec::with_capacity(args.len());
                        for arg in args {
                            let ty = self.resolve_ann(&scope, arg)?;
                            let is_frame_tvar = match &ty {
                                Type::TypeVarType {
                                    raw_id, namespace, ..
                                } => {
                                    *namespace == frame.fullname
                                        && frame.tvars.iter().any(|t| t.raw_id == *raw_id)
                                }
                                _ => false,
                            };
                            if !is_frame_tvar {
                                return Err(input(
                                    &self.path,
                                    base.line,
                                    "a `Generic[...]` argument must be a class type variable",
                                ));
                            }
                            resolved.push(ty);
                        }
                        base_mros.push(GENERIC_MRO.iter().map(|s| s.to_string()).collect());
                        bases.push(("typing.Generic".to_string(), resolved));
                    }
                    Some(Binding::Class(full)) => {
                        let base_model = self.base_model(head, full, base.line)?;
                        if args.len() != base_model.tvars.len() {
                            return Err(input(
                                &self.path,
                                base.line,
                                &format!(
                                    "the base `{head}` expects {} type arguments, got {}",
                                    base_model.tvars.len(),
                                    args.len()
                                ),
                            ));
                        }
                        let mut resolved = Vec::with_capacity(args.len());
                        for arg in args {
                            resolved.push(self.resolve_ann(&scope, arg)?);
                        }
                        base_mros.push(base_model.mro.clone());
                        bases.push((full.clone(), resolved));
                    }
                    _ => {
                        return Err(input(
                            &self.path,
                            base.line,
                            &format!("the base `{head}` is not a generic class in the subset"),
                        ))
                    }
                },
            }
        }
        if !bases.iter().any(|(r, _)| r == "builtins.object") {
            bases.push(("builtins.object".to_string(), Vec::new()));
            base_mros.push(OBJECT_MRO.iter().map(|s| s.to_string()).collect());
        }
        Ok((bases, base_mros))
    }

    fn base_model(
        &self,
        name: &str,
        fullname: &str,
        line: usize,
    ) -> Result<&ClassModel, CheckError> {
        let model_ref = self.classes.get(fullname).ok_or_else(|| {
            input(
                &self.path,
                line,
                &format!("the base `{name}` resolves outside the corpus"),
            )
        })?;
        Ok(model_ref)
    }

    /// Pass 1 for a method: resolve and register its signature. The
    /// body checks later, in `check_method_body`.
    fn bind_method(
        &mut self,
        bindings: &mut HashMap<String, Binding>,
        frame: &ClassFrame,
        class_fullname: &str,
        func: &FuncDefStmt,
    ) -> Result<(), CheckError> {
        let sig = {
            let scope = Scope {
                module: bindings,
                locals: None,
                frame: Some(frame),
            };
            self.resolve_sig(&scope, func)?
        };
        self.register_member(class_fullname, &func.name, Member::Method(sig), func.line)
    }

    /// Pass 1 for a module function: bind its signature only. The
    /// body checks later, in `check_function_body`.
    fn bind_function(
        &mut self,
        bindings: &mut HashMap<String, Binding>,
        func: &FuncDefStmt,
    ) -> Result<(), CheckError> {
        self.reject_module_rebind(bindings, &func.name, func.line)?;
        let sig = {
            let scope = Scope {
                module: bindings,
                locals: None,
                frame: None,
            };
            self.resolve_sig(&scope, func)?
        };
        bindings.insert(func.name.clone(), Binding::Function(sig));
        Ok(())
    }

    /// Pass 3: check a deferred module-function body against the
    /// full module bindings.
    fn check_function_body(
        &mut self,
        bindings: &HashMap<String, Binding>,
        func: &FuncDefStmt,
    ) -> Result<(), CheckError> {
        let sig = match bindings.get(&func.name) {
            Some(Binding::Function(sig)) => sig.clone(),
            _ => {
                return Err(CheckError::Internal(format!(
                    "the binding for the function `{}` is missing",
                    func.name
                )))
            }
        };
        let mut locals: HashMap<String, Type> = HashMap::new();
        for (pname, ptype) in &sig.params {
            locals.insert(pname.clone(), ptype.clone());
        }
        self.check_body(bindings, None, &sig.ret, &mut locals, &func.body, func.line)
    }

    /// Pass 3: check a deferred method body against the full module
    /// bindings and the class model, then the override check (the
    /// body-then-override order mypy uses).
    fn check_method_body(
        &mut self,
        bindings: &HashMap<String, Binding>,
        class_fullname: &str,
        func: &FuncDefStmt,
    ) -> Result<(), CheckError> {
        let (tvars, sig) = {
            let model_ref = self.classes.get(class_fullname).ok_or_else(|| {
                CheckError::Internal(format!("the class model for {class_fullname} is missing"))
            })?;
            match model_ref.members.get(&func.name) {
                Some(Member::Method(sig)) => (model_ref.tvars.clone(), sig.clone()),
                _ => {
                    return Err(CheckError::Internal(format!(
                        "the method `{}` is missing from the model of {class_fullname}",
                        func.name
                    )))
                }
            }
        };
        let frame = ClassFrame {
            fullname: class_fullname.to_string(),
            tvars,
        };
        let self_ty = {
            let model_ref = self.classes.get(class_fullname).ok_or_else(|| {
                CheckError::Internal(format!("the class model for {class_fullname} is missing"))
            })?;
            instance(class_fullname, model::class_frame_tvars(model_ref))
        };
        let mut locals: HashMap<String, Type> = HashMap::new();
        locals.insert("self".to_string(), self_ty);
        for (pname, ptype) in &sig.params {
            locals.insert(pname.clone(), ptype.clone());
        }
        self.check_body(
            bindings,
            Some(&frame),
            &sig.ret,
            &mut locals,
            &func.body,
            func.line,
        )?;
        self.check_override(class_fullname, &func.name, &sig, func.line)
    }

    /// Pass 3 driver: walk the module in statement order and check
    /// the deferred bodies. Statements with no body have nothing to
    /// check here.
    fn check_top_bodies(
        &mut self,
        bindings: &HashMap<String, Binding>,
        module: &str,
        kind: &StmtKind,
    ) -> Result<(), CheckError> {
        match kind {
            StmtKind::FuncDef(func) => self.check_function_body(bindings, func)?,
            StmtKind::ClassDef(cls) => {
                let fullname = format!("{module}.{}", cls.name);
                for stmt in &cls.body {
                    if let ClassStmt::Method(func) = stmt {
                        self.check_method_body(bindings, &fullname, func)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// One collection sweep over one class: type every `self.<attr> =
    /// ...` whose attribute is neither an own member nor
    /// base-defined, apply the candidates in statement order, and
    /// splice the model once when any applied. Returns whether a
    /// candidate applied; the caller loops until false.
    fn collect_class_sweep_once(
        &mut self,
        bindings: &HashMap<String, Binding>,
        cls: &crate::subset::ClassDefStmt,
        fullname: &str,
    ) -> Result<bool, CheckError> {
        let candidates = self.collect_class_candidates(bindings, cls, fullname)?;
        let mut applied = false;
        for (attr, vt) in candidates {
            let Some(model_ref) = self.classes.get_mut(fullname) else {
                return Err(CheckError::Internal(format!(
                    "the class model for {fullname} is missing"
                )));
            };
            if model_ref.members.contains_key(&attr) {
                continue;
            }
            model_ref.members.insert(attr, Member::InstanceVar(vt));
            applied = true;
        }
        if applied {
            self.refresh(fullname)?;
        }
        Ok(applied)
    }

    /// Pass 2 over the module: retry the classes' pending
    /// `self.<attr> = ...` candidates against the full module
    /// bindings. A body may read any module name (#126), so a value
    /// the statement-order collection could not type yet (it names a
    /// later function, variable or class) gets another sweep here.
    /// Monotone: every sweep only adds members, so it terminates.
    fn collect_instance_vars(
        &mut self,
        bindings: &HashMap<String, Binding>,
        module: &str,
        body: &[crate::subset::TopStmt],
    ) -> Result<(), CheckError> {
        let mut applied_any = true;
        while applied_any {
            applied_any = false;
            for stmt in body {
                let StmtKind::ClassDef(cls) = &stmt.kind else {
                    continue;
                };
                let fullname = format!("{module}.{}", cls.name);
                if self.collect_class_sweep_once(bindings, cls, &fullname)? {
                    applied_any = true;
                }
            }
        }
        // Drop provisional own attributes that shadow a base member, so
        // pass 3 checks the override against the base (#139 review, #93).
        for stmt in body {
            let StmtKind::ClassDef(cls) = &stmt.kind else {
                continue;
            };
            let fullname = format!("{module}.{}", cls.name);
            self.repair_shadowed_base_attrs(cls, &fullname)?;
        }
        Ok(())
    }

    /// Repair pass-1 misses against the base members the pass-2 sweeps
    /// collect. An own instance attribute shadowing a base member is
    /// provisional and is dropped, so pass 3 checks the override against
    /// the base. An own class variable shadowing a base instance
    /// attribute escaped the pass-1 VarDecl guard (the base member was
    /// not registered yet), so it is checked here: mypy allows a
    /// covariant override and rejects the rest as an assignment error,
    /// which the subset loudly rejects.
    fn repair_shadowed_base_attrs(
        &mut self,
        cls: &crate::subset::ClassDefStmt,
        fullname: &str,
    ) -> Result<(), CheckError> {
        let lines: HashMap<String, usize> = cls
            .body
            .iter()
            .filter_map(|stmt| match stmt {
                ClassStmt::VarDecl { name, line, .. } => Some((name.clone(), *line)),
                _ => None,
            })
            .collect();
        let (inst_names, classvars, receiver) = {
            let model_ref = self.classes.get(fullname).ok_or_else(|| {
                CheckError::Internal(format!("the class model for {fullname} is missing"))
            })?;
            let inst_names: Vec<String> = model_ref
                .members
                .iter()
                .filter(|(_, member)| matches!(member, Member::InstanceVar(_)))
                .map(|(name, _)| name.clone())
                .collect();
            let classvars: Vec<(String, Type)> = model_ref
                .members
                .iter()
                .filter_map(|(name, member)| match member {
                    Member::ClassVar(t) => Some((name.clone(), t.clone())),
                    _ => None,
                })
                .collect();
            let receiver = instance(fullname, model::class_frame_tvars(model_ref));
            (inst_names, classvars, receiver)
        };
        let mut dropped = false;
        for name in inst_names {
            if self
                .find_member(&receiver, &name, Some(fullname))?
                .is_some()
            {
                let model_ref = self.classes.get_mut(fullname).ok_or_else(|| {
                    CheckError::Internal(format!("the class model for {fullname} is missing"))
                })?;
                model_ref.members.remove(&name);
                dropped = true;
            }
        }
        if dropped {
            self.refresh(fullname)?;
        }
        for (name, own_ty) in classvars {
            let Some(Found::Var(base_ty)) = self.find_member(&receiver, &name, Some(fullname))?
            else {
                continue;
            };
            let line = lines.get(&name).copied().ok_or_else(|| {
                CheckError::Internal(format!(
                    "the declaration line for the class variable {fullname}.{name} is missing"
                ))
            })?;
            let verdict = require_decidable(
                self.sub(&own_ty, &base_ty),
                "class attribute override",
                &self.path,
                line,
            )?;
            if !verdict {
                return Err(input(
                    &self.path,
                    line,
                    "overriding a base class member is outside the skeleton subset",
                ));
            }
        }
        Ok(())
    }

    /// One sweep over one class: walk every method body and type each
    /// `self.<attr> = ...` whose attribute is neither an own member
    /// nor base-defined (those subtype against the existing member
    /// in pass 3, so they must not be re-registered).
    fn collect_class_candidates(
        &self,
        bindings: &HashMap<String, Binding>,
        cls: &crate::subset::ClassDefStmt,
        fullname: &str,
    ) -> Result<Vec<(String, Type)>, CheckError> {
        let model_ref = self.classes.get(fullname).ok_or_else(|| {
            CheckError::Internal(format!("the class model for {fullname} is missing"))
        })?;
        let frame = ClassFrame {
            fullname: fullname.to_string(),
            tvars: model_ref.tvars.clone(),
        };
        let ctx = CollectCtx {
            bindings,
            frame: &frame,
            self_ty: instance(fullname, model::class_frame_tvars(model_ref)),
            model: model_ref,
        };
        let mut candidates = Vec::new();
        for stmt in &cls.body {
            let ClassStmt::Method(func) = stmt else {
                continue;
            };
            let Some(Member::Method(sig)) = ctx.model.members.get(&func.name) else {
                continue;
            };
            let mut locals: HashMap<String, Type> = HashMap::new();
            locals.insert("self".to_string(), ctx.self_ty.clone());
            for (pname, ptype) in &sig.params {
                locals.insert(pname.clone(), ptype.clone());
            }
            self.collect_body_candidates(&ctx, &mut locals, &func.body, &mut candidates)?;
        }
        Ok(candidates)
    }

    /// The per-body walk of one sweep: collect self-attribute
    /// candidates and mirror the local-assignment typing so later
    /// candidates see the same names pass 3 will. A typing failure is
    /// skipped, never an error: pass 3 re-types the statement and
    /// rejects it with the construct-naming message.
    fn collect_body_candidates(
        &self,
        ctx: &CollectCtx,
        locals: &mut HashMap<String, Type>,
        seq: &[BodyStmt],
        candidates: &mut Vec<(String, Type)>,
    ) -> Result<(), CheckError> {
        for stmt in seq {
            let scope = Scope {
                module: ctx.bindings,
                locals: Some(&*locals),
                frame: Some(ctx.frame),
            };
            match stmt {
                BodyStmt::SelfAssign { attr, value, .. } => {
                    if ctx.model.members.contains_key(attr)
                        || self.find_member(&ctx.self_ty, attr, None)?.is_some()
                    {
                        continue;
                    }
                    if let Ok(vt) = self.type_expr(&scope, value) {
                        candidates.push((attr.clone(), vt));
                    }
                }
                BodyStmt::LocalAnnAssign {
                    name, ann, value, ..
                } => {
                    if let Ok(declared) = self.resolve_ann(&scope, ann) {
                        if self.type_expr(&scope, value).is_ok() {
                            locals.insert(name.clone(), declared);
                        }
                    }
                }
                BodyStmt::LocalAssign { name, value, .. } => {
                    if let Ok(t) = self.type_expr(&scope, value) {
                        locals.insert(name.clone(), t);
                    }
                }
                BodyStmt::If {
                    branches,
                    else_body,
                    ..
                } => {
                    for (_, body) in branches {
                        self.collect_body_candidates(ctx, locals, body, candidates)?;
                    }
                    if let Some(else_body) = else_body {
                        self.collect_body_candidates(ctx, locals, else_body, candidates)?;
                    }
                }
                BodyStmt::Call(_) | BodyStmt::Return { .. } | BodyStmt::Pass => {}
            }
        }
        Ok(())
    }

    fn resolve_sig(&self, scope: &Scope, func: &FuncDefStmt) -> Result<Sig, CheckError> {
        let mut params = Vec::with_capacity(func.params.len());
        for p in &func.params {
            params.push((p.name.clone(), self.resolve_ann(scope, &p.ann)?));
        }
        let ret = self.resolve_ann(scope, &func.ret)?;
        Ok(Sig { params, ret })
    }

    /// Check a function body: every statement against the locals and the
    /// return annotation, then the all-paths-return analysis. A body of a
    /// non-None function that can fall off the end is mypy's
    /// "Missing return statement" error: rendered for the main file, a
    /// hard reject for an imported sibling (the renderer owns one path).
    fn check_body(
        &mut self,
        bindings: &HashMap<String, Binding>,
        frame: Option<&ClassFrame>,
        ret: &Type,
        locals: &mut HashMap<String, Type>,
        body: &[BodyStmt],
        def_line: usize,
    ) -> Result<(), CheckError> {
        self.check_seq(bindings, frame, ret, locals, body)?;
        if matches!(ret, Type::NoneType) || always_returns(body) {
            return Ok(());
        }
        if self.is_main {
            self.diagnostics.push(Diagnostic {
                line: def_line,
                message: "error: Missing return statement  [return]".to_string(),
            });
            Ok(())
        } else {
            Err(input(
                &self.path,
                def_line,
                "a missing return statement is outside the supported error classes",
            ))
        }
    }

    /// Walk one body sequence. `locals` is mutated only by top-level
    /// assignments; branch bodies never assign locals (a lowering rule),
    /// so the branch recursion shares the same map. Each statement gets
    /// a fresh scope so a name assigned earlier in the sequence resolves.
    fn check_seq(
        &mut self,
        bindings: &HashMap<String, Binding>,
        frame: Option<&ClassFrame>,
        ret: &Type,
        locals: &mut HashMap<String, Type>,
        seq: &[BodyStmt],
    ) -> Result<(), CheckError> {
        for stmt in seq {
            let scope = Scope {
                module: bindings,
                locals: Some(&*locals),
                frame,
            };
            match stmt {
                BodyStmt::SelfAssign { attr, value, line } => {
                    let vt = self.type_expr(&scope, value)?;
                    let self_ty = locals.get("self").ok_or_else(|| {
                        CheckError::Internal("a self assignment outside a method".to_string())
                    })?;
                    match self.find_member(self_ty, attr, None)? {
                        Some(Found::Var(t)) => {
                            let verdict = require_decidable(
                                self.sub(&vt, &t),
                                "instance attribute assignment",
                                &self.path,
                                *line,
                            )?;
                            if !verdict {
                                return Err(input(
                                    &self.path,
                                    *line,
                                    "instance attribute assignment incompatibility is \
                                     outside the supported error classes",
                                ));
                            }
                        }
                        Some(Found::Method(_)) => {
                            return Err(input(
                                &self.path,
                                *line,
                                "assigning to a method is outside the skeleton subset",
                            ))
                        }
                        None => {
                            return Err(CheckError::Internal(
                                "an instance attribute that the collection pass did not \
                                 register"
                                    .to_string(),
                            ))
                        }
                    }
                }
                BodyStmt::Call(expr) => {
                    self.type_expr(&scope, expr)?;
                }
                BodyStmt::Return {
                    value: Some(v),
                    line,
                } => {
                    let vt = self.type_expr(&scope, v)?;
                    let verdict =
                        require_decidable(self.sub(&vt, ret), "return-value", &self.path, *line)?;
                    if !verdict {
                        self.incompatible_return_value(&vt, ret, *line)?;
                    }
                }
                BodyStmt::Return { value: None, .. } | BodyStmt::Pass => {}
                BodyStmt::LocalAnnAssign {
                    name,
                    ann,
                    value,
                    line,
                } => {
                    let declared = self.resolve_ann(&scope, ann)?;
                    let expr_t = self.type_expr(&scope, value)?;
                    let verdict = require_decidable(
                        self.sub(&expr_t, &declared),
                        "local assignment",
                        &self.path,
                        *line,
                    )?;
                    if !verdict {
                        self.incompatible_assignment(&expr_t, &declared, *line)?;
                    }
                    locals.insert(name.clone(), declared);
                }
                BodyStmt::LocalAssign { name, value, .. } => {
                    let t = self.type_expr(&scope, value)?;
                    locals.insert(name.clone(), t);
                }
                BodyStmt::If {
                    branches,
                    else_body,
                    ..
                } => {
                    // mypy only type-checks the condition (no bool
                    // requirement; `truthy-bool` is not a default error
                    // code), so the result is discarded.
                    for (cond, _) in branches {
                        self.type_expr(&scope, cond)?;
                    }
                    for (_, body) in branches {
                        self.check_seq(bindings, frame, ret, locals, body)?;
                    }
                    if let Some(else_body) = else_body {
                        self.check_seq(bindings, frame, ret, locals, else_body)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// A returned value that is not a subtype of the annotation: the
    /// rendered return-value error, and only when both sides are
    /// builtins instances (the fixture-covered primitives); every other
    /// incompatibility is out of subset.
    fn incompatible_return_value(
        &mut self,
        expr_t: &Type,
        declared: &Type,
        line: usize,
    ) -> Result<(), CheckError> {
        if self.is_main {
            if let (Type::Instance { type_ref: e, .. }, Type::Instance { type_ref: v, .. }) =
                (expr_t, declared)
            {
                if e.starts_with("builtins.") && v.starts_with("builtins.") {
                    self.diagnostics.push(Diagnostic {
                        line,
                        message: format!(
                            "error: Incompatible return value type (got \"{expr}\", \
                             expected \"{var}\")  [return-value]",
                            expr = display_type(e),
                            var = display_type(v),
                        ),
                    });
                    return Ok(());
                }
            }
        }
        Err(input(
            &self.path,
            line,
            "return-value incompatibility is outside the supported error classes",
        ))
    }

    /// The method-override check: every base definer in the MRO (mypy
    /// checks each, not just the nearest) must accept the override's
    /// parameters and return a supertype. `__init__`, `__new__`,
    /// `__init_subclass__` and `__post_init__` are exempt, matching
    /// mypy's checker.py exemption list.
    fn check_override(
        &self,
        class_fullname: &str,
        name: &str,
        sig: &Sig,
        line: usize,
    ) -> Result<(), CheckError> {
        if matches!(
            name,
            "__init__" | "__new__" | "__init_subclass__" | "__post_init__"
        ) {
            return Ok(());
        }
        let model_ref = self.classes.get(class_fullname).ok_or_else(|| {
            CheckError::Internal(format!("the class model for {class_fullname} is missing"))
        })?;
        let receiver = instance(class_fullname, model::class_frame_tvars(model_ref));
        let mut skip = Some(class_fullname.to_string());
        while let Some((found, definer)) =
            self.find_member_entry(&receiver, name, skip.as_deref())?
        {
            match found {
                Found::Method(base_sig) => {
                    if base_sig.params.len() != sig.params.len() {
                        return Err(input(
                            &self.path,
                            line,
                            "a method override with a different parameter count is outside \
                             the supported error classes",
                        ));
                    }
                    for ((_, sub_p), (_, super_p)) in sig.params.iter().zip(&base_sig.params) {
                        let verdict = require_decidable(
                            self.sub(super_p, sub_p),
                            "override parameter",
                            &self.path,
                            line,
                        )?;
                        if !verdict {
                            return Err(input(
                                &self.path,
                                line,
                                "method override incompatibility is outside \
                                 the supported error classes",
                            ));
                        }
                    }
                    let verdict = require_decidable(
                        self.sub(&sig.ret, &base_sig.ret),
                        "override return",
                        &self.path,
                        line,
                    )?;
                    if !verdict {
                        return Err(input(
                            &self.path,
                            line,
                            "method override incompatibility is outside \
                             the supported error classes",
                        ));
                    }
                    skip = Some(definer);
                }
                Found::Var(_) => {
                    return Err(input(
                        &self.path,
                        line,
                        "overriding a non-method member with a method is outside \
                         the supported error classes",
                    ))
                }
            }
        }
        Ok(())
    }

    /// Resolve one annotation to a type.
    fn resolve_ann(&self, scope: &Scope, ann: &Ann) -> Result<Type, CheckError> {
        match &ann.kind {
            AnnKind::NoneT => Ok(Type::NoneType),
            AnnKind::Name(name) => self.resolve_name_ann(scope, name, ann.line),
            AnnKind::Subscript { base, args } => {
                self.resolve_class_subscript(scope, base, args, ann.line)
            }
        }
    }

    fn resolve_name_ann(&self, scope: &Scope, name: &str, line: usize) -> Result<Type, CheckError> {
        if let Some(frame) = scope.frame {
            if let Some(info) = frame.tvars.iter().find(|t| t.binding == name) {
                return Ok(model::tvar_type(info, &frame.fullname));
            }
        }
        match scope.module.get(name) {
            Some(Binding::TypeVar { .. }) => Err(input(
                &self.path,
                line,
                "type variables are only supported as class type parameters",
            )),
            Some(Binding::Class(full)) => {
                let model_ref = self.classes.get(full).ok_or_else(|| {
                    CheckError::Internal(format!("the class model for {full} is missing"))
                })?;
                if model_ref.tvars.is_empty() {
                    Ok(instance(full, Vec::new()))
                } else {
                    Err(input(
                        &self.path,
                        line,
                        &format!("the generic class `{name}` must be subscripted in an annotation"),
                    ))
                }
            }
            Some(_) => Err(input(&self.path, line, &format!("`{name}` is not a type"))),
            None => match self.builtins.get(name) {
                Some(full) => Ok(instance(full, Vec::new())),
                None => Err(input(&self.path, line, &format!("`{name}` is not defined"))),
            },
        }
    }

    fn resolve_class_subscript(
        &self,
        scope: &Scope,
        base: &str,
        args: &[Ann],
        line: usize,
    ) -> Result<Type, CheckError> {
        let Some(Binding::Class(full)) = scope.module.get(base) else {
            return Err(input(
                &self.path,
                line,
                &format!("`{base}` is not a generic class in the subset"),
            ));
        };
        let model_ref = self.classes.get(full).ok_or_else(|| {
            CheckError::Internal(format!("the class model for {full} is missing"))
        })?;
        if args.len() != model_ref.tvars.len() {
            return Err(input(
                &self.path,
                line,
                &format!(
                    "`{base}` expects {} type arguments, got {}",
                    model_ref.tvars.len(),
                    args.len()
                ),
            ));
        }
        let mut resolved = Vec::with_capacity(args.len());
        for arg in args {
            resolved.push(self.resolve_ann(scope, arg)?);
        }
        Ok(instance(full, resolved))
    }

    /// Type one expression of the supported subset.
    fn type_expr(&self, scope: &Scope, expr: &Expr) -> Result<Type, CheckError> {
        match &expr.kind {
            ExprKind::Lit(lit) => Ok(instance(lit.type_fullname(), Vec::new())),
            ExprKind::Name(name) => self.type_name(scope, name, expr.line),
            ExprKind::Super => Err(input(
                &self.path,
                expr.line,
                "super() is only supported as a call target",
            )),
            ExprKind::Attr { obj, name } => self.type_attr(scope, obj, name, expr.line),
            ExprKind::Subscript { base, args } => {
                let item = self.resolve_class_subscript(scope, base, args, expr.line)?;
                Ok(Type::TypeType {
                    item: Box::new(item),
                    is_type_form: false,
                })
            }
            ExprKind::Call { func, args } => self.type_call(scope, func, args, expr.line),
            ExprKind::BinOp { op, left, right } => {
                self.type_binop(scope, op, left, right, expr.line)
            }
            ExprKind::Compare { op, left, right } => {
                self.type_compare(scope, op, left, right, expr.line)
            }
            ExprKind::BoolOp { op, operands } => self.type_boolop(scope, op, operands),
            ExprKind::Not { operand } => {
                // mypy accepts any operand type for `not` (the result is
                // always bool); only the operand must type-check.
                self.type_expr(scope, operand)?;
                Ok(instance("builtins.bool", Vec::new()))
            }
        }
    }

    fn type_name(&self, scope: &Scope, name: &str, line: usize) -> Result<Type, CheckError> {
        if let Some(t) = scope.locals.and_then(|l| l.get(name)) {
            return Ok(t.clone());
        }
        match scope.module.get(name) {
            Some(Binding::Var(t)) => Ok(t.clone()),
            Some(Binding::Class(full)) => {
                let model_ref = self.classes.get(full).ok_or_else(|| {
                    CheckError::Internal(format!("the class model for {full} is missing"))
                })?;
                Ok(Type::TypeType {
                    item: Box::new(instance(full, model::class_frame_tvars(model_ref))),
                    is_type_form: false,
                })
            }
            Some(Binding::Function(_)) => Err(input(
                &self.path,
                line,
                "a function reference is only supported as a direct call target",
            )),
            Some(Binding::TypeVar { .. }) => Err(input(
                &self.path,
                line,
                "a type variable in value position is outside the skeleton subset",
            )),
            Some(Binding::Module) => Err(input(
                &self.path,
                line,
                "module references are outside the skeleton subset",
            )),
            Some(Binding::Marker) => Err(input(
                &self.path,
                line,
                "typing names are only supported in type positions",
            )),
            None => match self.builtins.get(name) {
                Some(full) => Ok(Type::TypeType {
                    item: Box::new(instance(full, Vec::new())),
                    is_type_form: false,
                }),
                None => Err(input(&self.path, line, &format!("`{name}` is not defined"))),
            },
        }
    }

    /// Attribute read: instance members resolve through the kernel
    /// snapshots; class objects read class variables of corpus classes.
    fn type_attr(
        &self,
        scope: &Scope,
        obj: &Expr,
        name: &str,
        line: usize,
    ) -> Result<Type, CheckError> {
        if matches!(obj.kind, ExprKind::Super) {
            return Err(input(
                &self.path,
                line,
                "super() is only supported as a call target",
            ));
        }
        let obj_ty = self.type_expr(scope, obj)?;
        match obj_ty {
            Type::Instance { ref type_ref, .. } => match self.find_member(&obj_ty, name, None)? {
                Some(Found::Var(t)) => Ok(t),
                Some(Found::Method(_)) => Err(input(
                    &self.path,
                    line,
                    &format!("the method `{name}` is only supported as a call target"),
                )),
                None => Err(input(
                    &self.path,
                    line,
                    &format!("{type_ref} has no attribute `{name}` in the subset"),
                )),
            },
            Type::TypeType { item, .. } => match *item {
                Type::Instance { type_ref, args, .. } => {
                    let model_ref = self.classes.get(&type_ref).ok_or_else(|| {
                        input(
                            &self.path,
                            line,
                            &format!(
                                "class attribute access on `{type_ref}` is outside the subset"
                            ),
                        )
                    })?;
                    // Conservative (#127): parameterized class objects
                    // would need frame_env substitution first; reject so
                    // no bare tvar reaches the kernel.
                    if !args.is_empty() {
                        return Err(input(
                            &self.path,
                            line,
                            "reading a class variable through a parameterized class \
                             object is outside the skeleton subset",
                        ));
                    }
                    match model_ref.members.get(name) {
                        Some(Member::ClassVar(t)) => Ok(t.clone()),
                        Some(_) => Err(input(
                            &self.path,
                            line,
                            "reading a non-class-variable through a class object \
                             is outside the skeleton subset",
                        )),
                        None => Err(input(
                            &self.path,
                            line,
                            "reading a class variable this class does not define \
                             is outside the skeleton subset",
                        )),
                    }
                }
                _ => Err(input(
                    &self.path,
                    line,
                    "class attribute access on this expression is outside the skeleton subset",
                )),
            },
            _ => Err(input(
                &self.path,
                line,
                "attribute access on this expression is outside the skeleton subset",
            )),
        }
    }

    fn type_call(
        &self,
        scope: &Scope,
        func: &Expr,
        args: &[Expr],
        line: usize,
    ) -> Result<Type, CheckError> {
        let mut arg_types = Vec::with_capacity(args.len());
        for arg in args {
            arg_types.push(self.type_expr(scope, arg)?);
        }
        match &func.kind {
            ExprKind::Name(name) => {
                if scope.locals.and_then(|l| l.get(name)).is_some() {
                    return Err(input(
                        &self.path,
                        line,
                        "calling a local variable is outside the skeleton subset",
                    ));
                }
                match scope.module.get(name) {
                    Some(Binding::Class(full)) => self.construct(full, None, &arg_types, line),
                    Some(Binding::Function(sig)) => {
                        self.check_call_sig(sig, &arg_types, line)?;
                        Ok(sig.ret.clone())
                    }
                    Some(_) => Err(input(
                        &self.path,
                        line,
                        &format!("calling `{name}` is outside the skeleton subset"),
                    )),
                    None => {
                        if name == "str" && self.builtins.contains_key("str") {
                            return self.construct_str(&arg_types, line);
                        }
                        Err(input(
                            &self.path,
                            line,
                            &format!("calling `{name}` is outside the skeleton subset"),
                        ))
                    }
                }
            }
            ExprKind::Attr { obj, name } if matches!(obj.kind, ExprKind::Super) => {
                let self_ty = scope.locals.and_then(|l| l.get("self")).ok_or_else(|| {
                    input(
                        &self.path,
                        line,
                        "super() outside a method is outside the supported subset",
                    )
                })?;
                let frame_full = scope.frame.map(|f| f.fullname.clone()).ok_or_else(|| {
                    input(
                        &self.path,
                        line,
                        "super() outside a method is outside the supported subset",
                    )
                })?;
                match self.find_member(self_ty, name, Some(&frame_full))? {
                    Some(Found::Method(sig)) => {
                        self.check_call_sig(&sig, &arg_types, line)?;
                        Ok(sig.ret.clone())
                    }
                    Some(Found::Var(_)) => Err(input(
                        &self.path,
                        line,
                        "attribute reads through super() are outside the skeleton subset",
                    )),
                    None => Err(input(
                        &self.path,
                        line,
                        &format!("super() has no attribute `{name}` in the subset"),
                    )),
                }
            }
            ExprKind::Attr { obj, name } => {
                let obj_ty = self.type_expr(scope, obj)?;
                match obj_ty {
                    Type::Instance { .. } => match self.find_member(&obj_ty, name, None)? {
                        Some(Found::Method(sig)) => {
                            self.check_call_sig(&sig, &arg_types, line)?;
                            Ok(sig.ret.clone())
                        }
                        Some(Found::Var(_)) => Err(input(
                            &self.path,
                            line,
                            &format!(
                                "calling the attribute `{name}` is outside the skeleton subset"
                            ),
                        )),
                        None => Err(input(
                            &self.path,
                            line,
                            &format!("no callable attribute `{name}` in the subset"),
                        )),
                    },
                    Type::TypeType { .. } => Err(input(
                        &self.path,
                        line,
                        "unbound method calls on a class object are outside \
                         the skeleton subset",
                    )),
                    _ => Err(input(
                        &self.path,
                        line,
                        "calling a member on this expression is outside the skeleton subset",
                    )),
                }
            }
            ExprKind::Subscript { base, args: targs } => {
                let item = self.resolve_class_subscript(scope, base, targs, line)?;
                let Type::Instance {
                    type_ref,
                    args: resolved,
                    ..
                } = &item
                else {
                    return Err(CheckError::Internal(
                        "a class subscript resolved to a non-instance".to_string(),
                    ));
                };
                self.construct(type_ref, Some(resolved.clone()), &arg_types, line)
            }
            _ => Err(input(
                &self.path,
                line,
                "this call target is outside the skeleton subset",
            )),
        }
    }

    /// `str(x)`: one argument, any instance, always `builtins.str`. The
    /// argument check is an `is_subtype` consult against object so the
    /// constructor stays inside the kernel's closure.
    fn construct_str(&self, arg_types: &[Type], line: usize) -> Result<Type, CheckError> {
        if arg_types.len() != 1 {
            return Err(input(
                &self.path,
                line,
                "the str constructor takes exactly one argument in the subset",
            ));
        }
        let verdict = require_decidable(
            self.sub(&arg_types[0], &object_type()),
            "str constructor argument",
            &self.path,
            line,
        )?;
        if !verdict {
            return Err(input(
                &self.path,
                line,
                "argument incompatibility in the str constructor is outside \
                 the supported error classes",
            ));
        }
        Ok(instance("builtins.str", Vec::new()))
    }

    /// Construct a corpus class: resolve or infer the class type
    /// arguments, then check `__init__`'s substituted signature.
    fn construct(
        &self,
        fullname: &str,
        explicit: Option<Vec<Type>>,
        arg_types: &[Type],
        line: usize,
    ) -> Result<Type, CheckError> {
        let model_ref = self.classes.get(fullname).ok_or_else(|| {
            input(
                &self.path,
                line,
                &format!("constructing `{fullname}` is outside the skeleton subset"),
            )
        })?;
        let receiver = instance(fullname, model::class_frame_tvars(model_ref));
        let Some((init, _)) = self.find_corpus_method(&receiver, "__init__")? else {
            return Err(input(
                &self.path,
                line,
                "constructing a class without an __init__ is outside \
                 the skeleton subset",
            ));
        };
        let class_args = match explicit {
            Some(args) => {
                if args.len() != model_ref.tvars.len() {
                    return Err(input(
                        &self.path,
                        line,
                        &format!(
                            "the generic class `{fullname}` expects {} type arguments, got {}",
                            model_ref.tvars.len(),
                            args.len()
                        ),
                    ));
                }
                args
            }
            None => {
                if init.params.len() != arg_types.len() {
                    return Err(input(
                        &self.path,
                        line,
                        "an argument count mismatch is outside the skeleton subset",
                    ));
                }
                let mut inferred = Vec::with_capacity(model_ref.tvars.len());
                for tvar in &model_ref.tvars {
                    let needle = model::tvar_type(tvar, fullname);
                    let mut bound: Option<Type> = None;
                    for ((_, p), a) in init.params.iter().zip(arg_types.iter()) {
                        if bound.is_none() && *p == needle {
                            bound = Some(a.clone());
                        }
                    }
                    let Some(arg) = bound else {
                        return Err(input(
                            &self.path,
                            line,
                            "generic constructor inference is outside the skeleton subset",
                        ));
                    };
                    inferred.push(arg);
                }
                inferred
            }
        };
        let env = frame_env(model_ref, &class_args).map_err(CheckError::Internal)?;
        let sig = subst_sig(&init, &env);
        self.check_call_sig(&sig, arg_types, line)?;
        Ok(instance(fullname, class_args))
    }

    fn check_call_sig(&self, sig: &Sig, arg_types: &[Type], line: usize) -> Result<(), CheckError> {
        if sig.params.len() != arg_types.len() {
            return Err(input(
                &self.path,
                line,
                "an argument count mismatch is outside the skeleton subset",
            ));
        }
        for ((_, p), a) in sig.params.iter().zip(arg_types) {
            let verdict = require_decidable(self.sub(a, p), "argument", &self.path, line)?;
            if !verdict {
                return Err(input(
                    &self.path,
                    line,
                    "argument incompatibility is outside the supported error classes",
                ));
            }
        }
        Ok(())
    }

    /// Binary `+`/`*`/`%`: the operand-pair result table decides which
    /// pairs the slice supports first (unsupported pairs are out of
    /// subset, whatever the closure holds), then the operator member
    /// must exist in the left operand's snapshot closure (a kernel
    /// consult, never a pure table).
    fn type_binop(
        &self,
        scope: &Scope,
        op: &BinOpKind,
        left: &Expr,
        right: &Expr,
        line: usize,
    ) -> Result<Type, CheckError> {
        let lt = self.type_expr(scope, left)?;
        let rt = self.type_expr(scope, right)?;
        let Type::Instance { type_ref: lref, .. } = &lt else {
            return Err(input(
                &self.path,
                line,
                "binary operations on non-instance operands are outside \
                 the skeleton subset",
            ));
        };
        let result = match (&lt, &rt) {
            (Type::Instance { type_ref: a, .. }, Type::Instance { type_ref: b, .. }) => {
                match (a.as_str(), b.as_str(), op) {
                    ("builtins.str", "builtins.str", BinOpKind::Add) => {
                        instance("builtins.str", Vec::new())
                    }
                    (
                        "builtins.float",
                        "builtins.float",
                        BinOpKind::Add | BinOpKind::Mult | BinOpKind::Mod,
                    ) => instance("builtins.float", Vec::new()),
                    ("builtins.int", "builtins.int", BinOpKind::Mod) => {
                        instance("builtins.int", Vec::new())
                    }
                    _ => {
                        return Err(input(
                            &self.path,
                            line,
                            "binary operations on these types are outside the supported subset",
                        ))
                    }
                }
            }
            _ => {
                return Err(input(
                    &self.path,
                    line,
                    "binary operations on these types are outside the supported subset",
                ))
            }
        };
        let member = match op {
            BinOpKind::Add => "__add__",
            BinOpKind::Mult => "__mul__",
            BinOpKind::Mod => "__mod__",
        };
        self.require_snapshot_member(lref, member, line)?;
        Ok(result)
    }

    /// A comparison: both operands must be builtins int/float/bool
    /// instances (the pairs the corpus uses, result `builtins.bool` as
    /// in mypy), and the operator member must exist in the left
    /// operand's snapshot closure (a kernel consult, never a table).
    fn type_compare(
        &self,
        scope: &Scope,
        op: &CmpOpKind,
        left: &Expr,
        right: &Expr,
        line: usize,
    ) -> Result<Type, CheckError> {
        let lt = self.type_expr(scope, left)?;
        let rt = self.type_expr(scope, right)?;
        let (Type::Instance { type_ref: lref, .. }, Type::Instance { type_ref: rref, .. }) =
            (&lt, &rt)
        else {
            return Err(input(
                &self.path,
                line,
                "comparison operations on non-instance operands are outside \
                 the skeleton subset",
            ));
        };
        if !is_comparison_operand(lref) || !is_comparison_operand(rref) {
            return Err(input(
                &self.path,
                line,
                "comparison operations on these types are outside the supported subset",
            ));
        }
        let member = match op {
            CmpOpKind::Eq => "__eq__",
            CmpOpKind::NotEq => "__ne__",
            CmpOpKind::Lt => "__lt__",
            CmpOpKind::LtE => "__le__",
            CmpOpKind::Gt => "__gt__",
            CmpOpKind::GtE => "__ge__",
        };
        self.require_snapshot_member(lref, member, line)?;
        Ok(instance("builtins.bool", Vec::new()))
    }

    /// `and`/`or` over `builtins.bool` operands: mypy's join semantics
    /// are unmodeled, so any non-bool operand is out of subset; the
    /// result is bool.
    fn type_boolop(
        &self,
        scope: &Scope,
        op: &BoolOpKind,
        operands: &[Expr],
    ) -> Result<Type, CheckError> {
        let op_name = match op {
            BoolOpKind::And => "and",
            BoolOpKind::Or => "or",
        };
        for operand in operands {
            let t = self.type_expr(scope, operand)?;
            if !matches!(&t, Type::Instance { type_ref, .. } if type_ref == "builtins.bool") {
                return Err(input(
                    &self.path,
                    operand.line,
                    &format!("boolean `{op_name}` is only supported over builtins.bool operands"),
                ));
            }
        }
        Ok(instance("builtins.bool", Vec::new()))
    }

    /// The operator-member kernel consult shared by binops and
    /// comparisons: the dunder must be defined somewhere in the operand's
    /// snapshot MRO walk. A missing snapshot or member is an internal
    /// error (the closure broke), never a subset rejection.
    fn require_snapshot_member(
        &self,
        type_ref: &str,
        member: &str,
        line: usize,
    ) -> Result<(), CheckError> {
        let snap = self.resolver.get(type_ref).ok_or_else(|| {
            CheckError::Internal(format!(
                "{}:{line}: the snapshot for {type_ref} is missing; the fixture closure \
                 no longer covers the corpus",
                self.path
            ))
        })?;
        let mut found = false;
        for entry in &snap.mro {
            let Some(entry_snap) = self.resolver.get(entry) else {
                if entry == "typing.Generic" {
                    // Outside the fixtures on purpose: no corpus member
                    // resolves there.
                    continue;
                }
                return Err(CheckError::Internal(format!(
                    "{}:{line}: the snapshot for {entry} is missing; the fixture \
                     closure no longer covers the corpus",
                    self.path
                )));
            };
            if entry_snap.member_definers.contains_key(member) {
                found = true;
                break;
            }
        }
        if !found {
            return Err(CheckError::Internal(format!(
                "{}:{line}: the operator member `{member}` of {type_ref} fell outside \
                 the snapshot closure",
                self.path
            )));
        }
        Ok(())
    }

    /// Resolve a member to its substituted shape plus the fullname of
    /// the defining class, through the kernel snapshots: walk the
    /// receiver's MRO, consult `member_definers`, and substitute the
    /// definer's member with the receiver's type arguments. `skip_class`
    /// starts the walk after that MRO entry (the `super()` lookup, and
    /// the override check's walk past the previous definer).
    fn find_member_entry(
        &self,
        receiver: &Type,
        name: &str,
        skip_class: Option<&str>,
    ) -> Result<Option<(Found, String)>, CheckError> {
        let Type::Instance {
            type_ref: recv_ref,
            args: recv_args,
            ..
        } = receiver
        else {
            return Err(CheckError::Internal(
                "a member lookup on a non-instance receiver".to_string(),
            ));
        };
        let snap = self.resolver.get(recv_ref).ok_or_else(|| {
            CheckError::Internal(format!(
                "{}: the snapshot for {recv_ref} is missing; the fixture closure \
                 no longer covers the corpus",
                self.path
            ))
        })?;
        let mut skipping = skip_class.map(|s| s.to_string());
        for entry in &snap.mro {
            if let Some(skip) = &skipping {
                if entry == skip {
                    skipping = None;
                }
                continue;
            }
            let Some(entry_snap) = self.resolver.get(entry) else {
                if entry == "typing.Generic" {
                    // Outside the fixtures on purpose: no corpus member
                    // resolves there.
                    continue;
                }
                return Err(CheckError::Internal(format!(
                    "{}: the snapshot for {entry} is missing; the fixture closure \
                     no longer covers the corpus",
                    self.path
                )));
            };
            let Some((_, definer)) = entry_snap.member_definers.get(name) else {
                continue;
            };
            if definer != entry {
                return Err(CheckError::Internal(format!(
                    "{}: the snapshot of {entry} names {definer} as the definer of `{name}`",
                    self.path
                )));
            }
            let model_ref = self.classes.get(definer).ok_or_else(|| {
                CheckError::Input(format!(
                    "{}: skeleton subset error: the member `{name}` resolves to a class \
                     outside the subset ({definer})",
                    self.path
                ))
            })?;
            let Some(member) = model_ref.members.get(name) else {
                return Err(CheckError::Internal(format!(
                    "{}: the snapshot of {entry} and the model of {definer} disagree \
                     about `{name}`",
                    self.path
                )));
            };
            let args_at = self
                .map_args(recv_ref, recv_args, definer)?
                .ok_or_else(|| {
                    CheckError::Internal(format!(
                        "{}: no inheritance path from {recv_ref} to {definer}",
                        self.path
                    ))
                })?;
            let env = frame_env(model_ref, &args_at).map_err(CheckError::Internal)?;
            let found = match member {
                Member::Method(sig) => Found::Method(subst_sig(sig, &env)),
                Member::ClassVar(t) | Member::InstanceVar(t) => Found::Var(subst(t, &env)),
            };
            return Ok(Some((found, definer.to_string())));
        }
        Ok(None)
    }

    /// Resolve a member: the entry lookup's substituted shape alone.
    fn find_member(
        &self,
        receiver: &Type,
        name: &str,
        skip_class: Option<&str>,
    ) -> Result<Option<Found>, CheckError> {
        Ok(self
            .find_member_entry(receiver, name, skip_class)?
            .map(|(found, _)| found))
    }

    /// Resolve a method against corpus class models only: walk the
    /// receiver's MRO like `find_member_entry`, but skip MRO entries
    /// with no corpus model (builtins.object, typing.Generic) instead
    /// of surfacing their fixture-backed members. Callers whose
    /// intended diagnostic is "the corpus does not define this" get it
    /// instead of the fixture-walk's outside-the-subset error.
    fn find_corpus_method(
        &self,
        receiver: &Type,
        name: &str,
    ) -> Result<Option<(Sig, String)>, CheckError> {
        let Type::Instance {
            type_ref: recv_ref,
            args: recv_args,
            ..
        } = receiver
        else {
            return Err(CheckError::Internal(
                "a member lookup on a non-instance receiver".to_string(),
            ));
        };
        let snap = self.resolver.get(recv_ref).ok_or_else(|| {
            CheckError::Internal(format!(
                "{}: the snapshot for {recv_ref} is missing; the fixture closure \
                 no longer covers the corpus",
                self.path
            ))
        })?;
        for entry in &snap.mro {
            let Some(model_ref) = self.classes.get(entry) else {
                continue;
            };
            let Some(Member::Method(sig)) = model_ref.members.get(name) else {
                continue;
            };
            let args_at = self.map_args(recv_ref, recv_args, entry)?.ok_or_else(|| {
                CheckError::Internal(format!(
                    "{}: no inheritance path from {recv_ref} to {entry}",
                    self.path
                ))
            })?;
            let env = frame_env(model_ref, &args_at).map_err(CheckError::Internal)?;
            return Ok(Some((subst_sig(sig, &env), entry.to_string())));
        }
        Ok(None)
    }

    /// Map the receiver's type arguments to the definer's frame: follow
    /// the model's bases, substituting outward, until `target` is
    /// reached. Non-corpus bases (typing.Generic, builtins.object) end
    /// the walk with `None`.
    fn map_args(
        &self,
        from_ref: &str,
        from_args: &[Type],
        target: &str,
    ) -> Result<Option<Vec<Type>>, CheckError> {
        if from_ref == target {
            return Ok(Some(from_args.to_vec()));
        }
        let Some(model_ref) = self.classes.get(from_ref) else {
            return Ok(None);
        };
        let env = frame_env(model_ref, from_args).map_err(CheckError::Internal)?;
        for (b_ref, b_args) in &model_ref.bases {
            let concrete: Vec<Type> = b_args.iter().map(|a| subst(a, &env)).collect();
            if b_ref == target {
                return Ok(Some(concrete));
            }
            if let Some(mapped) = self.map_args(b_ref, &concrete, target)? {
                return Ok(Some(mapped));
            }
        }
        Ok(None)
    }

    fn sub(&self, left: &Type, right: &Type) -> Option<bool> {
        is_subtype(left, right, &self.ctx, &self.resolver)
    }
}
