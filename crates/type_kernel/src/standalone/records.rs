//! Standalone-path public API: records.
//!
//! The semantic record layer for a Python-free check: how a
//! `TypeInfoSnapshot` / `TypeResolver` is *produced* from source facts,
//! instead of being decoded from a committed fixture blob
//! (`crates/mypy-rs-skel/src/fixtures.rs`) or read out of a live Python
//! `TypeInfo` graph (`typeinfo::snapshot_type_info`, which is what the
//! hybrid does once per pass).
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): every public item takes and returns pure-Rust kernel types
//! only. No pyo3 type appears in a public signature, nothing here
//! registers a seam, and no hybrid code path changes.
//!
//! What is lifted, and from where:
//!
//! * `typeinfo::snapshot_type_info` (typeinfo.rs:1321) fixes the record
//!   vocabulary: the fields a snapshot carries and their defaults.
//!   [`ClassFacts`] is its input-side mirror, minus the reads that only
//!   mean anything against a live Python object.
//! * `mro::rust_linearize_hierarchy` (mro.rs:156), the port of
//!   `mypy/mro.py::linearize_hierarchy`, computes the MRO. Records call
//!   it; C3 is not re-ported here.
//! * `mypy/nodes.py::TypeInfo.calculate_metaclass_type` (nodes.py:4227)
//!   plus `mypy/semanal.py::SemanticAnalyzer.recalculate_metaclass`
//!   (semanal.py:4008) decide `metaclass_fullname`. The protocol arm of
//!   the second one is already ported as the pure decision core
//!   `semanal_metaclass::classify_recalculate_metaclass_inner`
//!   (semanal_metaclass.rs:210), but that helper is module-private and
//!   its only public entry is a `#[pyfunction]` over a live `ClassDef`,
//!   so the six-line rule is restated in [`RecordStore::metaclass`]. A
//!   `pub(crate)` on the helper is what removes the restatement.
//! * `mypy/nodes.py::TypeInfo.protocol_members` (nodes.py:4119) decides
//!   `protocol_members`.
//! * `mypy/semanal_classprop.py::calculate_class_abstract_status`
//!   (semanal_classprop.py:60) decides `is_abstract`, and
//!   `add_type_promotion` (semanal_classprop.py:170) decides
//!   `promote_bytes`.
//! * `mypy/fixup.py::NodeFixer.visit_type_info` (ported at fixup.rs:575)
//!   is the fixup a record built in this run still needs: every base
//!   fullname must resolve before the MRO is linearized, and one that
//!   does not is an error, never a guess.
//!
//! Two facts the hybrid reads from live Python and a producer must
//! therefore state explicitly, because no snapshot field carries them:
//! `declared_metaclass` (a snapshot only carries the *calculated*
//! metaclass) and per-member `abstract_status` (a snapshot carries no
//! abstractness at all). Both are [`ClassFacts`] fields; without them the
//! metaclass and abstract-status calculations cannot run.
//!
//! Owned exclusively by the `records` lane for this wave. The
//! `#[cfg(test)]` module at the bottom is the only caller of this API
//! until the wave-2 integration lane wires it into the skeleton driver.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::aliases::TypeAliasResolver;
use crate::skeleton_api::encode_type;
use crate::typeinfo::{ModuleSnapshot, NativeTypeResolver, TypeInfoSnapshot, TypeResolver};
use crate::wire::{self, ReadBuffer, Type};

/// `mypy.nodes.NOT_ABSTRACT` (nodes.py:1185).
pub const NOT_ABSTRACT: i64 = 0;

/// `mypy.nodes.IS_ABSTRACT` (nodes.py:1187): set for an `@abstractmethod`
/// (semanal.py:2423) and for a Var marked abstract (semanal.py:5132).
pub const IS_ABSTRACT: i64 = 1;

/// `mypy.nodes.IMPLICITLY_ABSTRACT` (nodes.py:1189). A stub producer never
/// sets it: semanal.py:1566 gates the write on `not self.is_stub_file`,
/// because every body in a stub is trivially empty. It stays part of the
/// vocabulary so the abstract-status walk matches
/// `calculate_class_abstract_status` for non-stub input.
pub const IMPLICITLY_ABSTRACT: i64 = 2;

/// `EXCLUDED_PROTOCOL_ATTRIBUTES` (nodes.py:3729): the special names
/// `protocol_members` never collects.
pub const EXCLUDED_PROTOCOL_ATTRIBUTES: &[&str] = &[
    "__abstractmethods__",
    "__annotations__",
    "__class_getitem__",
    "__dict__",
    "__doc__",
    "__init__",
    "__module__",
    "__new__",
    "__slots__",
    "__subclasshook__",
    "__weakref__",
];

/// The `mypy.nodes` symbol kinds the record layer distinguishes. The first
/// three are the ones `node_kind` (typeinfo.rs:732) stores in
/// `TypeInfoSnapshot::member_definers`; the rest all map to -1 and are kept
/// separate because `protocol_members` (nodes.py:4128) skips exactly three
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    /// `FuncDef` or `OverloadedFuncDef`.
    Function,
    /// `Decorator`: a decorated function mypy did not fold into an
    /// `OverloadedFuncDef` (a lone `@property`, `@classmethod`,
    /// `@staticmethod`).
    Decorator,
    /// `Var`: a class-level annotation, with or without a value.
    Var,
    /// `TypeAlias`.
    TypeAlias,
    /// `TypeVarExpr` / `ParamSpecExpr` / `TypeVarTupleExpr`.
    TypeVarLike,
    /// `MypyFile`: a submodule bound in a class body.
    Module,
    /// Any other node, e.g. a nested `TypeInfo`.
    Other,
}

impl MemberKind {
    /// The `member_definers` value `read_member_definers` (typeinfo.rs:695)
    /// stores: 0 for a FuncBase, 1 for a Decorator, 2 for a Var, -1 for
    /// everything else. A -1 member is dropped from `member_definers` but
    /// kept in `member_info`.
    pub fn node_kind(self) -> i64 {
        match self {
            MemberKind::Function => 0,
            MemberKind::Decorator => 1,
            MemberKind::Var => 2,
            _ => -1,
        }
    }

    /// `isinstance(node.node, (TypeAlias, TypeVarExpr, MypyFile))`, the
    /// auxiliary-definition test `protocol_members` skips (nodes.py:4128).
    pub fn is_auxiliary(self) -> bool {
        matches!(
            self,
            MemberKind::TypeAlias | MemberKind::TypeVarLike | MemberKind::Module
        )
    }
}

/// One member of a class body: the mirror of one `SymbolTableNode` in
/// `TypeInfo.names`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberFact {
    /// The symbol's node kind.
    pub kind: MemberKind,
    /// `SymbolTableNode.implicit`. A stub class body never sets it; it
    /// stays in the vocabulary because `member_info` carries it.
    pub implicit: bool,
    /// `Var.has_explicit_value`, false for every non-Var node, matching
    /// the defensive default in `read_member_info` (typeinfo.rs:681).
    pub has_explicit_value: bool,
    /// `FuncDef.abstract_status`, or `IS_ABSTRACT` for a Var with
    /// `is_abstract_var` set. Read only by the abstract-status walk.
    pub abstract_status: i64,
}

impl MemberFact {
    /// A member of the given kind, every other fact at its default.
    pub fn of_kind(kind: MemberKind) -> Self {
        MemberFact {
            kind,
            implicit: false,
            has_explicit_value: false,
            abstract_status: NOT_ABSTRACT,
        }
    }

    /// A `FuncDef` or `OverloadedFuncDef` member: a plain `def` in a stub
    /// body, an `@overload` group, or a `@property` with a setter (mypy
    /// folds the last two into an `OverloadedFuncDef`).
    pub fn function() -> Self {
        MemberFact::of_kind(MemberKind::Function)
    }

    /// A `Decorator` member.
    pub fn decorator() -> Self {
        MemberFact::of_kind(MemberKind::Decorator)
    }

    /// A `Var` member; `has_explicit_value` says whether the annotation
    /// carries an assignment.
    pub fn var(has_explicit_value: bool) -> Self {
        MemberFact {
            has_explicit_value,
            ..MemberFact::of_kind(MemberKind::Var)
        }
    }

    /// Set the abstract status, e.g. for an `@abstractmethod`.
    pub fn with_abstract_status(mut self, status: i64) -> Self {
        self.abstract_status = status;
        self
    }
}

/// One base class of a record: `mypy.types.Instance` reduced to the two
/// parts a snapshot blob carries (`type_ref` and `args`).
#[derive(Debug, Clone, PartialEq)]
pub struct BaseFact {
    /// The base's fullname, which must already resolve in the store.
    pub fullname: String,
    /// The subscript arguments in source order: the subclass frame's
    /// copies of the base's type variables.
    pub args: Vec<Type>,
}

impl BaseFact {
    /// A bare base with no type arguments.
    pub fn plain(fullname: &str) -> Self {
        BaseFact {
            fullname: fullname.to_string(),
            args: Vec::new(),
        }
    }
}

/// One entry of `defn.type_vars`: the source of the three parallel
/// snapshot arrays (`type_vars_with_variance`, `type_var_upper_bounds`,
/// `type_var_raw_ids`).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeVarFact {
    /// The name passed to `TypeVar`: the kernel-facing name.
    pub name: String,
    /// 0 = INVARIANT, 1 = COVARIANT, 2 = CONTRAVARIANT,
    /// 3 = VARIANCE_NOT_READY (nodes.py:3146).
    pub variance: i64,
    /// 0 = TypeVarType, 1 = ParamSpecType, 2 = TypeVarTupleType.
    pub kind: i64,
    /// `TypeVarId.raw_id`; -1 is the "unreadable" sentinel that never
    /// matches a real raw id.
    pub raw_id: i64,
    /// `TypeVarType.upper_bound`. Only read for `kind == 0`; the snapshot
    /// stores an empty blob for the other two kinds, matching
    /// `skeleton_codegen.py::snapshot_type_info`.
    pub upper_bound: Option<Type>,
    /// `TypeVarTupleType.tuple_fallback`. Only read for `kind == 2`, and
    /// only for the first such entry (the codegen's `if fallback is None`
    /// guard).
    pub tuple_fallback: Option<Type>,
}

/// The source facts of one class: everything a `TypeInfoSnapshot` needs
/// that a producer can read off a stub or a lowered module, plus the two
/// facts no snapshot field carries (`declared_metaclass` and per-member
/// `abstract_status`).
///
/// Field set and defaults follow `snapshot_type_info` (typeinfo.rs:1321):
/// a fact a producer does not state stays at the value mypy's own
/// defensive reads produce for a freshly built class.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClassFacts {
    /// `TypeInfo.fullname`.
    pub fullname: String,
    /// `TypeInfo.name`; empty means "derive from the fullname", the
    /// fallback `snapshot_type_info` applies (typeinfo.rs:1322).
    pub name: String,
    /// The declared bases in source order, *without* the implicit
    /// `builtins.object` base: [`RecordStore::insert_class`] adds that
    /// itself, mirroring semanal.py:3677.
    pub bases: Vec<BaseFact>,
    /// The class's own members. Inherited ones are not listed here; they
    /// are reached through the MRO, exactly as mypy reaches them through
    /// the `TypeInfo.names` of each MRO entry.
    pub members: BTreeMap<String, MemberFact>,
    /// `defn.type_vars`, in declaration order.
    pub type_vars: Vec<TypeVarFact>,
    /// `TypeInfo.is_protocol`.
    pub is_protocol: bool,
    /// `TypeInfo.is_enum`. The record layer ORs in the metaclass-derived
    /// value (semanal.py:4048), which only ever sets it to true.
    pub is_enum: bool,
    /// `TypeInfo.is_named_tuple`.
    pub is_named_tuple: bool,
    /// `TypeInfo.is_newtype`.
    pub is_newtype: bool,
    /// `TypeInfo.declared_metaclass` as a fullname: the metaclass the
    /// class body asked for with `metaclass=`, before
    /// `calculate_metaclass_type` walks the MRO. Not representable in a
    /// snapshot, and required to compute one.
    pub declared_metaclass: Option<String>,
    /// `TypeInfo.tuple_type`, for a NamedTuple class.
    pub tuple_type: Option<Type>,
    /// `TypeInfo._promote` targets stated explicitly, i.e. the
    /// `@_promote(...)` decorator arm of `add_type_promotion`
    /// (semanal_classprop.py:196). Empty means "consult the hardcoded
    /// promotion table".
    pub promote: Vec<Type>,
    /// `TypeInfo.alt_promote` fullname (the mypyc native-int arm).
    pub alt_promote_fullname: Option<String>,
    /// `TypeInfo.meta_fallback_to_any`.
    pub meta_fallback_to_any: bool,
    /// `TypeInfo.fallback_to_any` as declared; the inherited part is
    /// recomputed from the MRO by [`RecordStore::insert_class`]
    /// (mro.py:70).
    pub fallback_to_any: bool,
    /// `TypeInfo.type_var_tuple_prefix` (nodes.py:3895).
    pub type_var_tuple_prefix: Option<usize>,
    /// `TypeInfo.type_var_tuple_suffix` (nodes.py:3896).
    pub type_var_tuple_suffix: Option<usize>,
    /// `TypeInfo.enum_members` (nodes.py:3977).
    pub enum_members: Vec<String>,
    /// Whether the class body carried the typeshed-only `@disjoint_base`
    /// decorator (builtins.pyi:100 and friends). mypy keeps that fact on
    /// the `ClassDef`, not on the `TypeInfo`, so no snapshot field can
    /// carry it; the record layer keeps it here so a producer does not
    /// silently drop a semantic fact, and
    /// `tests::disjoint_base_changes_no_snapshot_field` pins that the
    /// snapshot cannot express it.
    pub disjoint_base: bool,
}

impl ClassFacts {
    /// Facts of a class that declares nothing but its name: no bases, no
    /// members, no type variables. A producer starts here and sets only
    /// what the source actually declares.
    pub fn new(fullname: &str) -> Self {
        ClassFacts {
            fullname: fullname.to_string(),
            ..ClassFacts::default()
        }
    }
}

/// The `mypy.nodes` symbol kinds a module symbol table can hold. Only
/// [`SymbolKind::Module`] is representable in a `ModuleSnapshot`, which
/// mirrors `snapshot_module` (typeinfo.rs:1445): the snapshot keeps a
/// fullname solely for the `MypyFile` case, because that is the only one
/// `rust_lookup_qualified` descends into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// `TypeInfo`.
    Class,
    /// `FuncDef` / `OverloadedFuncDef` / `Decorator`.
    Function,
    /// `Var`.
    Var,
    /// `MypyFile`.
    Module,
    /// `TypeAlias`.
    TypeAlias,
    /// `TypeVarExpr` / `ParamSpecExpr` / `TypeVarTupleExpr`.
    TypeVarLike,
    /// Anything else, including an unresolved placeholder.
    Other,
}

/// One entry of a module symbol table (`MypyFile.names`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolFact {
    /// The symbol's fullname: `module.name` for a class defined in the
    /// module, the imported fullname for a re-export.
    pub fullname: String,
    /// The node kind.
    pub kind: SymbolKind,
    /// `SymbolTableNode.module_hidden`.
    pub module_hidden: bool,
}

impl SymbolFact {
    /// A visible class symbol named `module.name`.
    pub fn class(module: &str, name: &str) -> Self {
        SymbolFact {
            fullname: qualified(module, name),
            kind: SymbolKind::Class,
            module_hidden: false,
        }
    }

    /// A visible non-class symbol named `module.name`.
    pub fn of_kind(module: &str, name: &str, kind: SymbolKind) -> Self {
        SymbolFact {
            fullname: qualified(module, name),
            kind,
            module_hidden: false,
        }
    }
}

/// `module.name`, or `name` when the module is empty.
pub fn qualified(module: &str, name: &str) -> String {
    if module.is_empty() {
        return name.to_string();
    }
    format!("{module}.{name}")
}

/// The source facts of one module's symbol table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleFacts {
    /// The module fullname, e.g. `builtins`.
    pub fullname: String,
    /// Every visible and hidden top-level name.
    pub symbols: BTreeMap<String, SymbolFact>,
}

impl ModuleFacts {
    /// The `ModuleSnapshot` projection of these facts: the shape
    /// `TypeResolver::get_module` answers with.
    pub fn snapshot(&self) -> ModuleSnapshot {
        let mut symbols = HashMap::with_capacity(self.symbols.len());
        for (name, fact) in &self.symbols {
            let module_node = match fact.kind {
                SymbolKind::Module => Some((true, fact.fullname.clone())),
                _ => None,
            };
            symbols.insert(name.clone(), (fact.module_hidden, module_node));
        }
        ModuleSnapshot { symbols }
    }

    /// The symbol `name` denotes, or `None` when it is absent or hidden.
    /// Hiding a name is what `ModuleSnapshot::visible` reports.
    pub fn resolve(&self, name: &str) -> Option<&SymbolFact> {
        let fact = self.symbols.get(name)?;
        if fact.module_hidden {
            return None;
        }
        Some(fact)
    }
}

/// The frame a name is looked up in. `mypy/semanal.py::lookup_qualified`
/// walks module and class frames; a function frame's *locals* are binder
/// state (`mypy/binder.py`), not a semantic record, so a function-scope
/// lookup answers from the frames the record layer owns and never invents
/// a local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolScope {
    /// A module frame: `MypyFile.names`.
    Module(String),
    /// A class frame: `TypeInfo.names`.
    Class(String),
    /// A function frame inside `module`, optionally inside a class. The
    /// function name is part of the scope's identity for diagnostics; the
    /// lookup itself resolves against the enclosing frames.
    Function {
        module: String,
        function: String,
        class: Option<String>,
    },
}

/// The answer of a symbol lookup: what the name denotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRef {
    /// The denoted fullname.
    pub fullname: String,
    /// The node kind.
    pub kind: SymbolKind,
    /// The frame that answered.
    pub scope: SymbolScope,
}

/// Why a record could not be produced. Every variant names the subject
/// and the fact that was missing or inconsistent; none of them is a
/// silent default, because a wrong record is a wrong check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    /// A fullname was asked for that no record was produced for.
    UnknownClass { fullname: String },
    /// A module was asked for that no symbol table was produced for.
    UnknownModule { fullname: String },
    /// A name is not in the module's symbol table, or is hidden.
    UnknownSymbol { module: String, name: String },
    /// A base fullname does not resolve: the fixup step
    /// (`mypy/fixup.py::lookup_typeinfo`) has no target for it.
    UnresolvedBase { class: String, base: String },
    /// A promotion target name does not denote a class.
    UnresolvedPromotion { class: String, name: String },
    /// The base list is cyclic. mypy reports "Cycle in inheritance
    /// hierarchy" (semanal.py:3700) and installs a dummy MRO; a producer
    /// has no dummy MRO to install, so it refuses the record.
    CyclicBases { class: String, cycle: Vec<String> },
    /// The same base appears twice. mypy reports `Duplicate base class`
    /// (semanal.py:3704) and installs an any-MRO; refused for the same
    /// reason as a cycle.
    DuplicateBase { class: String, base: String },
    /// C3 found no consistent linearization (`MroError`, mro.py:63).
    Unlinearizable { class: String },
    /// A wire blob could not be encoded or decoded.
    Wire { context: String, detail: String },
    /// Producing this record needs a semantic transformation the record
    /// layer does not implement; the reason names it.
    Unsupported { subject: String, reason: String },
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::UnknownClass { fullname } => {
                write!(f, "no class record was produced for `{fullname}`")
            }
            RecordError::UnknownModule { fullname } => {
                write!(f, "no symbol table was produced for module `{fullname}`")
            }
            RecordError::UnknownSymbol { module, name } => {
                write!(f, "`{module}` has no visible symbol `{name}`")
            }
            RecordError::UnresolvedBase { class, base } => {
                write!(f, "base `{base}` of `{class}` does not resolve to a record")
            }
            RecordError::UnresolvedPromotion { class, name } => {
                write!(f, "promotion target `{name}` of `{class}` is not a class")
            }
            RecordError::CyclicBases { class, cycle } => {
                write!(
                    f,
                    "cycle in the inheritance hierarchy of `{class}`: {cycle:?}"
                )
            }
            RecordError::DuplicateBase { class, base } => {
                write!(f, "duplicate base class `{base}` of `{class}`")
            }
            RecordError::Unlinearizable { class } => {
                write!(f, "no consistent MRO for `{class}`")
            }
            RecordError::Wire { context, detail } => {
                write!(f, "wire format error in {context}: {detail}")
            }
            RecordError::Unsupported { subject, reason } => {
                write!(f, "cannot produce a record for `{subject}`: {reason}")
            }
        }
    }
}

impl std::error::Error for RecordError {}

fn unknown_class(fullname: &str) -> RecordError {
    RecordError::UnknownClass {
        fullname: fullname.to_string(),
    }
}

fn unknown_module(fullname: &str) -> RecordError {
    RecordError::UnknownModule {
        fullname: fullname.to_string(),
    }
}

fn unknown_symbol(module: &str, name: &str) -> RecordError {
    RecordError::UnknownSymbol {
        module: module.to_string(),
        name: name.to_string(),
    }
}

fn unresolved_base(class: &str, base: &str) -> RecordError {
    RecordError::UnresolvedBase {
        class: class.to_string(),
        base: base.to_string(),
    }
}

fn unresolved_promotion(class: &str, name: &str) -> RecordError {
    RecordError::UnresolvedPromotion {
        class: class.to_string(),
        name: name.to_string(),
    }
}

fn cyclic_bases(class: &str, base: &str) -> RecordError {
    RecordError::CyclicBases {
        class: class.to_string(),
        cycle: vec![class.to_string(), base.to_string()],
    }
}

fn duplicate_base(class: &str, base: &str) -> RecordError {
    RecordError::DuplicateBase {
        class: class.to_string(),
        base: base.to_string(),
    }
}

fn unlinearizable(class: &str) -> RecordError {
    RecordError::Unlinearizable {
        class: class.to_string(),
    }
}

fn unsupported(subject: &str, reason: &str) -> RecordError {
    RecordError::Unsupported {
        subject: subject.to_string(),
        reason: reason.to_string(),
    }
}

fn wire_error(context: &str, detail: String) -> RecordError {
    RecordError::Wire {
        context: context.to_string(),
        detail,
    }
}

/// The hardcoded ad-hoc subtyping edges of
/// `mypy/semanal_classprop.py::TYPE_PROMOTIONS`
/// (semanal_classprop.py:52). The target is a *name in the class's own
/// module*, resolved through that module's symbol table the way
/// `add_type_promotion` resolves it through `module_names`.
///
/// The same table already exists in the kernel as the module-private
/// `semanal_classprop::TYPE_PROMOTIONS` (semanal_classprop.rs:14), which
/// only a `#[pyfunction]` over live Python objects reads. Making that
/// constant `pub(crate)` would let this copy go; until then
/// `tests::promotion_table_matches_the_python_source` pins the entries.
///
/// Two of the four are option-gated in mypy
/// (`Options.disable_bytearray_promotion`,
/// `Options.disable_memoryview_promotion`). The standalone path takes no
/// `Options`, so a producer covering `builtins.bytearray` or
/// `builtins.memoryview` must state the promotion explicitly in
/// [`ClassFacts::promote`] instead of relying on this table. Neither class
/// is in the #151 closure.
pub fn promotion_target(fullname: &str) -> Option<&'static str> {
    match fullname {
        "builtins.int" => Some("float"),
        "builtins.float" => Some("complex"),
        "builtins.bytearray" => Some("bytes"),
        "builtins.memoryview" => Some("bytes"),
        _ => None,
    }
}

/// The module part of a fullname: `builtins.int` lives in `builtins`.
pub fn module_of(fullname: &str) -> &str {
    match fullname.rsplit_once('.') {
        Some((module, _)) => module,
        None => "",
    }
}

/// The short name of a fullname: the `TypeInfo.name` fallback
/// `snapshot_type_info` applies (typeinfo.rs:1322).
pub fn short_name(fullname: &str) -> &str {
    match fullname.rsplit_once('.') {
        Some((_, name)) => name,
        None => fullname,
    }
}

/// A bare `mypy.types.Instance` of `fullname`.
pub fn instance_type(fullname: &str, args: &[Type]) -> Type {
    Type::Instance {
        type_ref: fullname.to_string(),
        args: args.to_vec(),
        last_known_value: None,
        extra_attrs: None,
    }
}

/// A produced record set: class snapshots, the module symbol tables they
/// were built against, and the source facts both were derived from.
///
/// Keeping the facts next to the snapshots is what makes the second-pass
/// calculations possible at all. `is_abstract` and `protocol_members` read
/// the per-member abstractness and `is_protocol` of *base* classes, and
/// `metaclass_fullname` reads their `declared_metaclass`; none of those
/// three survives into a `TypeInfoSnapshot`, so a store holding only
/// snapshots could not recompute them.
#[derive(Debug, Default)]
pub struct RecordStore {
    classes: BTreeMap<String, TypeInfoSnapshot>,
    facts: BTreeMap<String, ClassFacts>,
    modules: BTreeMap<String, ModuleFacts>,
}

impl RecordStore {
    /// An empty store.
    pub fn new() -> Self {
        RecordStore::default()
    }

    /// Number of class records held.
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    /// Whether no class record has been produced yet.
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// Install a module symbol table. Re-inserting a module replaces it.
    pub fn insert_module(&mut self, module: ModuleFacts) {
        self.modules.insert(module.fullname.clone(), module);
    }

    /// The symbol table of `fullname`, if one was installed.
    pub fn module(&self, fullname: &str) -> Option<&ModuleFacts> {
        self.modules.get(fullname)
    }

    /// Resolve `name` in module `fullname`. A missing module and a missing
    /// or hidden name are both errors: the callers (the promotion table,
    /// [`RecordStore::lookup`]) must not guess.
    pub fn lookup_module_symbol(
        &self,
        fullname: &str,
        name: &str,
    ) -> Result<&SymbolFact, RecordError> {
        let Some(module) = self.modules.get(fullname) else {
            return Err(unknown_module(fullname));
        };
        match module.resolve(name) {
            Some(fact) => Ok(fact),
            None => Err(unknown_symbol(fullname, name)),
        }
    }

    /// `mypy/semanal.py::lookup_qualified` for the frames the record layer
    /// owns. A module scope answers from the module symbol table, a class
    /// scope from the class's own members, and a function scope from its
    /// enclosing class (when it has one) or module: locals are binder
    /// state, not a record, so a name that only exists as a local answers
    /// `Ok(None)`.
    pub fn lookup(
        &self,
        scope: &SymbolScope,
        name: &str,
    ) -> Result<Option<SymbolRef>, RecordError> {
        let found = match scope {
            SymbolScope::Module(module) => self.module_lookup(module, name, scope)?,
            SymbolScope::Class(class) => self.class_lookup(class, name, scope)?,
            SymbolScope::Function { module, class, .. } => {
                let mut hit = None;
                if let Some(class) = class {
                    hit = self.class_lookup(class, name, scope)?;
                }
                if hit.is_none() {
                    let Some(found) = self.module(module) else {
                        return Err(unknown_module(module));
                    };
                    // A miss here may be a local, and locals are binder
                    // state, not a record: answer `None`, never an error.
                    if let Some(fact) = found.resolve(name) {
                        hit = Some(symbol_ref(fact, scope));
                    }
                }
                hit
            }
        };
        Ok(found)
    }

    fn module_lookup(
        &self,
        module: &str,
        name: &str,
        scope: &SymbolScope,
    ) -> Result<Option<SymbolRef>, RecordError> {
        let fact = self.lookup_module_symbol(module, name)?;
        Ok(Some(symbol_ref(fact, scope)))
    }

    fn class_lookup(
        &self,
        class: &str,
        name: &str,
        scope: &SymbolScope,
    ) -> Result<Option<SymbolRef>, RecordError> {
        let facts = self.class_facts(class)?;
        let Some(member) = facts.members.get(name) else {
            return Ok(None);
        };
        let found = SymbolRef {
            fullname: qualified(class, name),
            kind: member_symbol_kind(member.kind),
            scope: scope.clone(),
        };
        Ok(Some(found))
    }

    /// The class record for `fullname`.
    pub fn class(&self, fullname: &str) -> Option<&TypeInfoSnapshot> {
        self.classes.get(fullname)
    }

    /// The class record for `fullname`, or [`RecordError::UnknownClass`].
    pub fn resolve(&self, fullname: &str) -> Result<&TypeInfoSnapshot, RecordError> {
        match self.classes.get(fullname) {
            Some(snapshot) => Ok(snapshot),
            None => Err(unknown_class(fullname)),
        }
    }

    /// The source facts a class record was built from.
    pub fn facts(&self, fullname: &str) -> Option<&ClassFacts> {
        self.facts.get(fullname)
    }

    /// The MRO of a produced class record.
    pub fn mro(&self, fullname: &str) -> Result<&[String], RecordError> {
        let snapshot = self.resolve(fullname)?;
        Ok(snapshot.mro.as_slice())
    }

    /// Whether `base` is in the MRO of `fullname`: `TypeInfo.has_base`
    /// (nodes.py:4314) over the produced record.
    pub fn has_base(&self, fullname: &str, base: &str) -> Result<bool, RecordError> {
        let snapshot = self.resolve(fullname)?;
        Ok(snapshot.has_base(base))
    }

    /// The decoded `bases` blobs in declaration order: the query
    /// `map_instance_to_supertype` (maptype.py) and the MRO walk need.
    pub fn bases(&self, fullname: &str) -> Result<Vec<Type>, RecordError> {
        let snapshot = self.resolve(fullname)?;
        let mut decoded = Vec::with_capacity(snapshot.bases.len());
        for blob in &snapshot.bases {
            decoded.push(decode_type(blob, fullname)?);
        }
        Ok(decoded)
    }

    /// The decoded `_promote` targets.
    pub fn promote(&self, fullname: &str) -> Result<Vec<Type>, RecordError> {
        let snapshot = self.resolve(fullname)?;
        let mut decoded = Vec::with_capacity(snapshot.promote_bytes.len());
        for blob in &snapshot.promote_bytes {
            decoded.push(decode_type(blob, fullname)?);
        }
        Ok(decoded)
    }

    /// The decoded `tuple_type`, or `None` for a non-NamedTuple class.
    pub fn tuple_type(&self, fullname: &str) -> Result<Option<Type>, RecordError> {
        let snapshot = self.resolve(fullname)?;
        match &snapshot.tuple_type {
            Some(blob) => Ok(Some(decode_type(blob, fullname)?)),
            None => Ok(None),
        }
    }

    /// The decoded per-type-variable upper bounds, parallel to
    /// `type_vars_with_variance`. An entry is `None` where the snapshot
    /// carries an empty blob, which is what a ParamSpec or TypeVarTuple
    /// entry always is and what a TypeVar without a readable bound is.
    pub fn type_var_upper_bounds(&self, fullname: &str) -> Result<Vec<Option<Type>>, RecordError> {
        let snapshot = self.resolve(fullname)?;
        let bounds = &snapshot.type_var_upper_bounds;
        let mut decoded = Vec::with_capacity(bounds.len());
        for blob in bounds {
            if blob.is_empty() {
                decoded.push(None);
                continue;
            }
            decoded.push(Some(decode_type(blob, fullname)?));
        }
        Ok(decoded)
    }

    /// The variance of the class type variable called `name`, or `None`
    /// when the class has no such type variable. This is the
    /// `check_type_parameter` dispatch key (subtypes.py:1358).
    pub fn variance(&self, fullname: &str, name: &str) -> Result<Option<i64>, RecordError> {
        let snapshot = self.resolve(fullname)?;
        for entry in &snapshot.type_vars_with_variance {
            if entry.0 == name {
                return Ok(Some(entry.1));
            }
        }
        Ok(None)
    }

    /// The member fact of `name` in class `fullname`, or `None` when the
    /// class does not define it itself. Inherited members are found by
    /// walking [`RecordStore::mro`], as mypy does.
    pub fn member(&self, fullname: &str, name: &str) -> Result<Option<&MemberFact>, RecordError> {
        let facts = self.class_facts(fullname)?;
        Ok(facts.members.get(name))
    }

    /// The `TypeInfoSnapshot`s and `ModuleSnapshot`s of this store as the
    /// kernel's own resolver: the object `is_subtype` reads.
    pub fn resolver(&self) -> TypeResolver {
        let mut resolver = TypeResolver::new();
        for (fullname, snapshot) in &self.classes {
            resolver.insert(fullname.clone(), snapshot.clone());
        }
        for (fullname, module) in &self.modules {
            resolver.insert_module(fullname.clone(), module.snapshot());
        }
        resolver
    }

    /// Produce the record of one class and store it.
    ///
    /// This is the whole construction path, in mypy's order: the implicit
    /// `object` base (semanal.py:3677), the duplicate-base and cycle
    /// refusals (semanal.py:3821 and 3831), base fixup (the bases arm of
    /// fixup.rs:575), C3 linearization (mro.py:141), the inherited
    /// `fallback_to_any` (mro.py:70), the metaclass calculation and its
    /// protocol recalculation (nodes.py:4227, semanal.py:4008), the enum
    /// scan (semanal.py:4048), `protocol_members` (nodes.py:4119),
    /// `is_abstract` (semanal_classprop.py:60) and the promotion table
    /// (semanal_classprop.py:170).
    ///
    /// Re-inserting a fullname replaces the previous record, which is how
    /// a caller refreshes a class whose members grew after the first pass
    /// (the skeleton's `model::refresh_snapshot` does the same).
    pub fn insert_class(&mut self, facts: ClassFacts) -> Result<(), RecordError> {
        let snapshot = self.build_snapshot(&facts)?;
        self.classes.insert(facts.fullname.clone(), snapshot);
        self.facts.insert(facts.fullname.clone(), facts);
        Ok(())
    }

    /// The class facts of `fullname`, or [`RecordError::UnknownClass`].
    fn class_facts(&self, fullname: &str) -> Result<&ClassFacts, RecordError> {
        match self.facts.get(fullname) {
            Some(facts) => Ok(facts),
            None => Err(unknown_class(fullname)),
        }
    }

    /// The facts of a class that may be the one currently being built:
    /// `candidate` wins, because it is not in the store yet.
    fn facts_including<'a>(
        &'a self,
        fullname: &str,
        candidate: &'a ClassFacts,
    ) -> Result<&'a ClassFacts, RecordError> {
        if fullname == candidate.fullname {
            return Ok(candidate);
        }
        self.class_facts(fullname)
    }

    /// The snapshot of a class that may be the one currently being built;
    /// see [`RecordStore::facts_including`].
    fn snapshot_including<'a>(
        &'a self,
        fullname: &str,
        candidate: &'a TypeInfoSnapshot,
    ) -> Result<&'a TypeInfoSnapshot, RecordError> {
        if fullname == candidate.fullname {
            return Ok(candidate);
        }
        self.resolve(fullname)
    }

    fn build_snapshot(&self, facts: &ClassFacts) -> Result<TypeInfoSnapshot, RecordError> {
        if facts.fullname.is_empty() {
            return Err(unsupported("", "a class record needs a fullname"));
        }
        let bases = self.fixed_up_bases(facts)?;
        let base_blobs = encode_bases(facts, &bases)?;
        let mro = self.linearize(facts, &base_blobs)?;
        let has_base: HashSet<String> = mro.iter().cloned().collect();
        let name = if facts.name.is_empty() {
            short_name(&facts.fullname).to_string()
        } else {
            facts.name.clone()
        };
        let candidate = TypeInfoSnapshot {
            fullname: facts.fullname.clone(),
            name,
            mro: mro.clone(),
            has_base,
            bases: base_blobs,
            ..TypeInfoSnapshot::default()
        };
        let metaclass = self.metaclass(facts, &mro, &candidate)?;
        let is_enum = self.is_enum(facts, metaclass.as_deref(), &candidate)?;
        let protocol_members = self.protocol_members(facts, &mro)?;
        let is_abstract = self.is_abstract(facts, &mro)?;
        let promote_bytes = self.promote_bytes(facts)?;
        let fallback_to_any = self.fallback_to_any(facts, &mro, &candidate);
        let tuple_type = encode_option(facts.tuple_type.as_ref(), &facts.fullname, "tuple_type")?;
        let fallback = first_tuple_fallback(facts);
        let field = "type_var_tuple_fallback";
        let type_var_tuple_fallback = encode_option(fallback.as_ref(), &facts.fullname, field)?;
        let arrays = type_var_arrays(facts)?;
        let has_param_spec_type = facts.type_vars.iter().any(|tvar| tvar.kind == 1);
        let has_type_var_tuple_type = facts.type_vars.iter().any(|tvar| tvar.kind == 2);
        let (member_info, member_definers) = member_maps(facts);
        Ok(TypeInfoSnapshot {
            is_protocol: facts.is_protocol,
            is_enum,
            enum_members: facts.enum_members.clone(),
            fallback_to_any,
            meta_fallback_to_any: facts.meta_fallback_to_any,
            is_named_tuple: facts.is_named_tuple,
            is_newtype: facts.is_newtype,
            has_type_var_tuple_type,
            has_param_spec_type,
            is_abstract,
            type_vars: arrays.names,
            protocol_members,
            promote_bytes,
            alt_promote_fullname: facts.alt_promote_fullname.clone(),
            metaclass_fullname: Some(metaclass.unwrap_or_default()),
            tuple_type,
            type_var_tuple_prefix: facts.type_var_tuple_prefix,
            type_var_tuple_suffix: facts.type_var_tuple_suffix,
            type_var_tuple_fallback,
            type_vars_with_variance: arrays.with_variance,
            type_var_upper_bounds: arrays.upper_bounds,
            type_var_raw_ids: arrays.raw_ids,
            member_info,
            member_definers,
            ..candidate
        })
    }

    /// Base fixup: add the implicit `object` base, refuse duplicates and
    /// cycles, and check that every base resolves to a produced record.
    ///
    /// `mypy/fixup.py::NodeFixer.visit_type_info` (fixup.rs:575) resolves
    /// each base through `lookup_typeinfo` after a deserialize; a record
    /// built in this run has no serialized refs to fix, but the same
    /// invariant holds: a base that does not resolve is an error, never a
    /// silently shorter MRO.
    fn fixed_up_bases(&self, facts: &ClassFacts) -> Result<Vec<BaseFact>, RecordError> {
        let mut bases = facts.bases.clone();
        if bases.is_empty() && facts.fullname != "builtins.object" {
            // semanal.py:3677: "Add 'object' as implicit base if there is
            // no other base class."
            bases.push(BaseFact::plain("builtins.object"));
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for base in &bases {
            if !seen.insert(base.fullname.as_str()) {
                return Err(duplicate_base(&facts.fullname, &base.fullname));
            }
        }
        for base in &bases {
            let Some(snapshot) = self.classes.get(&base.fullname) else {
                return Err(unresolved_base(&facts.fullname, &base.fullname));
            };
            // semanal.py:3821 `verify_base_classes`: a base that is this
            // class, or whose MRO already holds it, is a cycle. Stored
            // MROs are complete, so one pass decides it.
            let self_base = base.fullname == facts.fullname;
            let in_base_mro = snapshot.mro.iter().any(|entry| *entry == facts.fullname);
            if self_base || in_base_mro {
                return Err(cyclic_bases(&facts.fullname, &base.fullname));
            }
        }
        Ok(bases)
    }

    /// C3 linearization through the kernel's own port of
    /// `mypy/mro.py::linearize_hierarchy`.
    ///
    /// `mro::linearize` itself is module-private and its public entry
    /// (`mro::rust_linearize_hierarchy`) takes an *owning*
    /// `NativeTypeResolver`, so the store is copied into a throwaway
    /// resolver per call. That is O(records) snapshot clones per class,
    /// which is noise next to parsing the stub the records came from; a
    /// `pub(crate)` on `mro::linearize` would remove the copy.
    fn linearize(
        &self,
        facts: &ClassFacts,
        base_blobs: &[Vec<u8>],
    ) -> Result<Vec<String>, RecordError> {
        let mut resolver = TypeResolver::new();
        for (fullname, snapshot) in &self.classes {
            resolver.insert(fullname.clone(), snapshot.clone());
        }
        let candidate = TypeInfoSnapshot {
            fullname: facts.fullname.clone(),
            bases: base_blobs.to_vec(),
            ..TypeInfoSnapshot::default()
        };
        let fullname = candidate.fullname.clone();
        resolver.insert(fullname.clone(), candidate);
        let native = NativeTypeResolver::new(resolver, TypeAliasResolver::new());
        match crate::mro::rust_linearize_hierarchy(&native, fullname.clone()) {
            Some(mro) => Ok(mro),
            None => Err(unlinearizable(&fullname)),
        }
    }

    /// `TypeInfo.calculate_metaclass_type` (nodes.py:4227) followed by the
    /// protocol arm of `SemanticAnalyzer.recalculate_metaclass`
    /// (semanal.py:4039). `None` means "no metaclass", which the snapshot
    /// spells as `Some("")`: the codegen maps Python's `None` metaclass to
    /// the empty string.
    fn metaclass(
        &self,
        facts: &ClassFacts,
        mro: &[String],
        candidate: &TypeInfoSnapshot,
    ) -> Result<Option<String>, RecordError> {
        let declared = match &facts.declared_metaclass {
            Some(fullname) => {
                let snapshot = self.snapshot_including(fullname, candidate)?;
                if !snapshot.has_base("builtins.type") {
                    // nodes.py:4229: a declared metaclass that is not
                    // itself a `type` subclass wins outright.
                    return Ok(Some(fullname.clone()));
                }
                Some(fullname.clone())
            }
            None => None,
        };
        if facts.fullname == "builtins.type" {
            // nodes.py:4232: `type` is its own metaclass.
            return Ok(Some("builtins.type".to_string()));
        }
        let mut winner = declared;
        for base in mro.iter().skip(1) {
            let base_facts = self.facts_including(base, facts)?;
            let Some(super_meta) = &base_facts.declared_metaclass else {
                continue;
            };
            let Some(current) = winner.clone() else {
                winner = Some(super_meta.clone());
                continue;
            };
            let current_snapshot = self.snapshot_including(&current, candidate)?;
            if current_snapshot.has_base(super_meta) {
                continue;
            }
            let super_snapshot = self.snapshot_including(super_meta, candidate)?;
            if super_snapshot.has_base(&current) {
                winner = Some(super_meta.clone());
                continue;
            }
            // nodes.py:4250: a metaclass conflict leaves no metaclass.
            winner = None;
            break;
        }
        let any_protocol = self.any_protocol_in_mro(facts, mro)?;
        let default_metaclass = match &winner {
            None => true,
            Some(fullname) => fullname == "builtins.type",
        };
        if any_protocol && default_metaclass {
            // semanal.py:4039: protocols and their subclasses get
            // abc.ABCMeta by default. A store without that record is
            // refused; the empty metaclass would be a wrong record.
            self.resolve("abc.ABCMeta")?;
            return Ok(Some("abc.ABCMeta".to_string()));
        }
        Ok(winner)
    }

    /// `any(info.is_protocol for info in defn.info.mro)` (semanal.py:4040)
    /// over the produced records.
    fn any_protocol_in_mro(&self, facts: &ClassFacts, mro: &[String]) -> Result<bool, RecordError> {
        for fullname in mro {
            let base_facts = self.facts_including(fullname, facts)?;
            if base_facts.is_protocol {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// semanal.py:4048: a metaclass under `enum.EnumMeta` makes the class
    /// an enum, and a generic enum is an error mypy reports ("Enum class
    /// cannot be generic"). A record layer cannot report it, so it
    /// refuses the record instead.
    fn is_enum(
        &self,
        facts: &ClassFacts,
        metaclass: Option<&str>,
        candidate: &TypeInfoSnapshot,
    ) -> Result<bool, RecordError> {
        let Some(fullname) = metaclass else {
            return Ok(facts.is_enum);
        };
        let snapshot = self.snapshot_including(fullname, candidate)?;
        if !snapshot.has_base("enum.EnumMeta") {
            return Ok(facts.is_enum);
        }
        if !facts.type_vars.is_empty() {
            let reason = "an enum class cannot be generic (semanal.py:4050)";
            return Err(unsupported(&facts.fullname, reason));
        }
        Ok(true)
    }

    /// `TypeInfo.protocol_members` (nodes.py:4119): the sorted union of the
    /// member names of every protocol in the MRO except `object`, minus the
    /// auxiliary definitions and [`EXCLUDED_PROTOCOL_ATTRIBUTES`].
    fn protocol_members(
        &self,
        facts: &ClassFacts,
        mro: &[String],
    ) -> Result<Vec<String>, RecordError> {
        let mut members: HashSet<&str> = HashSet::new();
        // nodes.py:4124 skips the MRO's last entry, which is `object`.
        for fullname in mro.iter().take(mro.len().saturating_sub(1)) {
            let base_facts = self.facts_including(fullname, facts)?;
            if !base_facts.is_protocol {
                continue;
            }
            for (name, member) in &base_facts.members {
                if member.kind.is_auxiliary() {
                    continue;
                }
                if EXCLUDED_PROTOCOL_ATTRIBUTES.contains(&name.as_str()) {
                    continue;
                }
                members.insert(name.as_str());
            }
        }
        let mut sorted: Vec<String> = Vec::with_capacity(members.len());
        for name in &members {
            sorted.push((*name).to_string());
        }
        sorted.sort();
        Ok(sorted)
    }

    /// `calculate_class_abstract_status` (semanal_classprop.py:60), the part
    /// that writes `is_abstract`: walk the MRO in order, and a member is
    /// abstract only while no earlier MRO entry made it concrete. The
    /// `abstract_attributes` list and the two stub/final error reports the
    /// Python function also produces are diagnostics: they belong to the
    /// `diag` area, not to a record.
    fn is_abstract(&self, facts: &ClassFacts, mro: &[String]) -> Result<bool, RecordError> {
        if facts.is_newtype {
            // semanal_classprop.py:75: a NewType is never abstract.
            return Ok(false);
        }
        let mut concrete: HashSet<&str> = HashSet::new();
        let mut is_abstract = false;
        for fullname in mro {
            let base_facts = self.facts_including(fullname, facts)?;
            for (name, member) in &base_facts.members {
                let status = match member.kind {
                    MemberKind::Function | MemberKind::Decorator | MemberKind::Var => {
                        member.abstract_status
                    }
                    _ => NOT_ABSTRACT,
                };
                let abstract_member = status == IS_ABSTRACT || status == IMPLICITLY_ABSTRACT;
                if abstract_member && !concrete.contains(name.as_str()) {
                    is_abstract = true;
                }
                concrete.insert(name.as_str());
            }
        }
        Ok(is_abstract)
    }

    /// `add_type_promotion` (semanal_classprop.py:170): the explicit
    /// `@_promote` targets if the source states any, otherwise the
    /// hardcoded table resolved through the class's own module symbol
    /// table. The mypyc native-int arm (semanal_classprop.py:222) is not
    /// reachable from a typeshed producer, and no stub in the #151 closure
    /// carries it.
    fn promote_bytes(&self, facts: &ClassFacts) -> Result<Vec<Vec<u8>>, RecordError> {
        if !facts.promote.is_empty() {
            let mut blobs = Vec::with_capacity(facts.promote.len());
            for target in &facts.promote {
                blobs.push(encode_type_blob(target, &facts.fullname, "_promote")?);
            }
            return Ok(blobs);
        }
        let Some(name) = promotion_target(&facts.fullname) else {
            return Ok(Vec::new());
        };
        let module = module_of(&facts.fullname);
        let symbol = self.lookup_module_symbol(module, name)?;
        if symbol.kind != SymbolKind::Class {
            return Err(unresolved_promotion(&facts.fullname, name));
        }
        let target = instance_type(&symbol.fullname, &[]);
        let blob = encode_type_blob(&target, &facts.fullname, "_promote")?;
        Ok(vec![blob])
    }

    /// The inherited half of `calculate_mro` (mro.py:70): "The property of
    /// falling back to Any is inherited."
    fn fallback_to_any(
        &self,
        facts: &ClassFacts,
        mro: &[String],
        candidate: &TypeInfoSnapshot,
    ) -> bool {
        if facts.fallback_to_any {
            return true;
        }
        for fullname in mro {
            let Ok(snapshot) = self.snapshot_including(fullname, candidate) else {
                continue;
            };
            if snapshot.fallback_to_any {
                return true;
            }
        }
        false
    }
}

/// The [`SymbolRef`] a module symbol answers with in `scope`.
fn symbol_ref(fact: &SymbolFact, scope: &SymbolScope) -> SymbolRef {
    SymbolRef {
        fullname: fact.fullname.clone(),
        kind: fact.kind,
        scope: scope.clone(),
    }
}

/// The member kind a class-frame symbol lookup answers with.
fn member_symbol_kind(kind: MemberKind) -> SymbolKind {
    match kind {
        MemberKind::Function | MemberKind::Decorator => SymbolKind::Function,
        MemberKind::Var => SymbolKind::Var,
        MemberKind::TypeAlias => SymbolKind::TypeAlias,
        MemberKind::TypeVarLike => SymbolKind::TypeVarLike,
        MemberKind::Module => SymbolKind::Module,
        MemberKind::Other => SymbolKind::Other,
    }
}

fn encode_type_blob(t: &Type, fullname: &str, field: &str) -> Result<Vec<u8>, RecordError> {
    match encode_type(t) {
        Ok(blob) => Ok(blob),
        Err(detail) => Err(wire_error(&qualified(fullname, field), detail)),
    }
}

fn encode_option(
    value: Option<&Type>,
    fullname: &str,
    field: &str,
) -> Result<Option<Vec<u8>>, RecordError> {
    match value {
        Some(t) => Ok(Some(encode_type_blob(t, fullname, field)?)),
        None => Ok(None),
    }
}

/// Encode the fixed-up bases as the wire-format `Instance` blobs
/// `TypeInfoSnapshot::bases` carries (nodes.py:3880).
fn encode_bases(facts: &ClassFacts, bases: &[BaseFact]) -> Result<Vec<Vec<u8>>, RecordError> {
    let mut blobs = Vec::with_capacity(bases.len());
    for base in bases {
        let base_type = instance_type(&base.fullname, &base.args);
        blobs.push(encode_type_blob(&base_type, &facts.fullname, "bases")?);
    }
    Ok(blobs)
}

fn decode_type(blob: &[u8], fullname: &str) -> Result<Type, RecordError> {
    let mut buf = ReadBuffer::new(blob);
    match wire::read_type(&mut buf, None) {
        Ok(t) => Ok(t),
        Err(e) => Err(wire_error(fullname, e.to_string())),
    }
}

/// The three parallel per-type-variable snapshot arrays plus the name
/// list, built in one pass so they cannot drift apart.
struct TypeVarArrays {
    names: Vec<String>,
    with_variance: Vec<(String, i64, i64)>,
    upper_bounds: Vec<Vec<u8>>,
    raw_ids: Vec<i64>,
}

/// Build [`TypeVarArrays`] the way `skeleton_codegen.py::snapshot_type_info`
/// builds the same three arrays: raw ids are positional and never skipped,
/// variance triples carry `(name, variance, kind)`, and an upper bound is
/// an empty blob for every non-TypeVar kind.
fn type_var_arrays(facts: &ClassFacts) -> Result<TypeVarArrays, RecordError> {
    let len = facts.type_vars.len();
    let mut arrays = TypeVarArrays {
        names: Vec::with_capacity(len),
        with_variance: Vec::with_capacity(len),
        upper_bounds: Vec::with_capacity(len),
        raw_ids: Vec::with_capacity(len),
    };
    for tvar in &facts.type_vars {
        arrays.names.push(tvar.name.clone());
        arrays.raw_ids.push(tvar.raw_id);
        let triple = (tvar.name.clone(), tvar.variance, tvar.kind);
        arrays.with_variance.push(triple);
        let bound = match tvar.kind {
            0 => tvar.upper_bound.as_ref(),
            _ => None,
        };
        match bound {
            Some(bound_type) => {
                let blob = encode_type_blob(bound_type, &facts.fullname, "upper_bound")?;
                arrays.upper_bounds.push(blob);
            }
            None => arrays.upper_bounds.push(Vec::new()),
        }
    }
    Ok(arrays)
}

/// `TypeVarTupleType.tuple_fallback` of the class's first variadic type
/// variable (types.py:991 and the codegen's `if fallback is None` guard),
/// or `None` when the class is not variadic.
fn first_tuple_fallback(facts: &ClassFacts) -> Option<Type> {
    for tvar in &facts.type_vars {
        if tvar.kind == 2 {
            return tvar.tuple_fallback.clone();
        }
    }
    None
}

/// `TypeInfo.names` as the two snapshot maps `read_member_info`
/// (typeinfo.rs:661) and `read_member_definers` (typeinfo.rs:695) produce:
/// every name in `member_info`, only the FuncBase / Decorator / Var names
/// in `member_definers`, and the definer of an own member is always the
/// class itself.
fn member_maps(
    facts: &ClassFacts,
) -> (
    HashMap<String, (bool, bool)>,
    HashMap<String, (i64, String)>,
) {
    let mut member_info = HashMap::with_capacity(facts.members.len());
    let mut member_definers = HashMap::new();
    for (name, member) in &facts.members {
        let info = (member.implicit, member.has_explicit_value);
        member_info.insert(name.clone(), info);
        let kind = member.kind.node_kind();
        if kind >= 0 {
            member_definers.insert(name.clone(), (kind, facts.fullname.clone()));
        }
    }
    (member_info, member_definers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push((*item).to_string());
        }
        out
    }

    fn with_bases(fullname: &str, bases: &[&str]) -> ClassFacts {
        let mut facts = ClassFacts::new(fullname);
        for base in bases {
            facts.bases.push(BaseFact::plain(base));
        }
        facts
    }

    /// `builtins.object` and `builtins.type`: the two records every other
    /// test needs, produced the way the stub producer produces them.
    fn root_store() -> RecordStore {
        let mut store = RecordStore::new();
        let object = ClassFacts::new("builtins.object");
        let type_info = with_bases("builtins.type", &["builtins.object"]);
        store.insert_class(object).unwrap();
        store.insert_class(type_info).unwrap();
        store
    }

    fn protocol_store() -> RecordStore {
        let mut store = root_store();
        let meta = with_bases("abc.ABCMeta", &["builtins.type"]);
        store.insert_class(meta).unwrap();
        let mut protocol = with_bases("typing.Container", &["builtins.object"]);
        protocol.is_protocol = true;
        let member = MemberFact::function().with_abstract_status(IS_ABSTRACT);
        protocol.members.insert("__contains__".to_string(), member);
        let alias = MemberFact::of_kind(MemberKind::TypeAlias);
        protocol.members.insert("Alias".to_string(), alias);
        let nested = MemberFact::of_kind(MemberKind::Other);
        protocol.members.insert("Nested".to_string(), nested);
        let excluded = MemberFact::function();
        protocol.members.insert("__init__".to_string(), excluded);
        store.insert_class(protocol).unwrap();
        store
    }

    fn builtins_classes(names: &[&str]) -> ModuleFacts {
        let mut module = ModuleFacts {
            fullname: "builtins".to_string(),
            symbols: BTreeMap::new(),
        };
        for name in names {
            let fact = SymbolFact::class("builtins", name);
            module.symbols.insert((*name).to_string(), fact);
        }
        module
    }

    fn add_member(facts: &mut ClassFacts, name: &str, member: MemberFact) {
        facts.members.insert(name.to_string(), member);
    }

    #[test]
    fn object_record_has_no_bases_and_a_single_entry_mro() {
        let store = root_store();
        let snapshot = store.class("builtins.object").unwrap();
        assert!(snapshot.bases.is_empty());
        let want = names(&["builtins.object"]);
        assert_eq!(snapshot.mro, want);
        assert_eq!(snapshot.name, "object");
        assert_eq!(snapshot.metaclass_fullname, Some(String::new()));
    }

    #[test]
    fn baseless_class_gets_the_implicit_object_base() {
        let mut store = root_store();
        let facts = ClassFacts::new("mymod.Plain");
        store.insert_class(facts).unwrap();
        let want = names(&["mymod.Plain", "builtins.object"]);
        assert_eq!(store.mro("mymod.Plain").unwrap().to_vec(), want);
        let bases = store.bases("mymod.Plain").unwrap();
        assert_eq!(bases, vec![instance_type("builtins.object", &[])]);
    }

    #[test]
    fn mro_is_c3_over_the_produced_records() {
        let mut store = root_store();
        store.insert_module(builtins_classes(&["float"]));
        let int = with_bases("builtins.int", &["builtins.object"]);
        store.insert_class(int).unwrap();
        let bool_facts = with_bases("builtins.bool", &["builtins.int"]);
        store.insert_class(bool_facts).unwrap();
        let want = names(&["builtins.bool", "builtins.int", "builtins.object"]);
        let got = store.mro("builtins.bool").unwrap().to_vec();
        assert_eq!(got, want);
        assert!(store.has_base("builtins.bool", "builtins.int").unwrap());
        assert!(!store.has_base("builtins.int", "builtins.bool").unwrap());
    }

    #[test]
    fn unlinearizable_hierarchy_is_refused() {
        let mut store = root_store();
        let a = with_bases("mymod.A", &["builtins.object"]);
        let b = with_bases("mymod.B", &["builtins.object"]);
        let x = with_bases("mymod.X", &["mymod.A", "mymod.B"]);
        let y = with_bases("mymod.Y", &["mymod.B", "mymod.A"]);
        let z = with_bases("mymod.Z", &["mymod.X", "mymod.Y"]);
        store.insert_class(a).unwrap();
        store.insert_class(b).unwrap();
        store.insert_class(x).unwrap();
        store.insert_class(y).unwrap();
        let err = store.insert_class(z).unwrap_err();
        assert_eq!(err, unlinearizable("mymod.Z"));
    }

    #[test]
    fn unresolved_base_is_refused_not_guessed() {
        let mut store = root_store();
        let facts = with_bases("mymod.C", &["mymod.Missing"]);
        let err = store.insert_class(facts).unwrap_err();
        assert_eq!(err, unresolved_base("mymod.C", "mymod.Missing"));
        assert!(store.class("mymod.C").is_none());
    }

    #[test]
    fn cyclic_base_list_is_refused_not_looped() {
        let mut store = root_store();
        let a = with_bases("mymod.A", &["builtins.object"]);
        store.insert_class(a).unwrap();
        let b = with_bases("mymod.B", &["mymod.A"]);
        store.insert_class(b).unwrap();
        let again = with_bases("mymod.A", &["mymod.B"]);
        let err = store.insert_class(again).unwrap_err();
        assert_eq!(err, cyclic_bases("mymod.A", "mymod.B"));
        let self_base = with_bases("mymod.Self", &["mymod.Self"]);
        let err = store.insert_class(self_base).unwrap_err();
        assert_eq!(err, unresolved_base("mymod.Self", "mymod.Self"));
    }

    #[test]
    fn duplicate_base_is_refused() {
        let mut store = root_store();
        let facts = with_bases("mymod.C", &["builtins.object", "builtins.object"]);
        let err = store.insert_class(facts).unwrap_err();
        assert_eq!(err, duplicate_base("mymod.C", "builtins.object"));
    }

    #[test]
    fn builtins_type_is_its_own_metaclass() {
        let store = root_store();
        let snapshot = store.class("builtins.type").unwrap();
        let want = Some("builtins.type".to_string());
        assert_eq!(snapshot.metaclass_fullname, want);
    }

    #[test]
    fn declared_metaclass_outside_the_type_hierarchy_wins() {
        let mut store = root_store();
        let meta = with_bases("mymod.Meta", &["builtins.object"]);
        store.insert_class(meta).unwrap();
        let mut facts = with_bases("mymod.C", &["builtins.object"]);
        facts.declared_metaclass = Some("mymod.Meta".to_string());
        store.insert_class(facts).unwrap();
        let snapshot = store.class("mymod.C").unwrap();
        let want = Some("mymod.Meta".to_string());
        assert_eq!(snapshot.metaclass_fullname, want);
    }

    #[test]
    fn protocol_in_the_mro_installs_abcmeta() {
        let mut store = protocol_store();
        let facts = with_bases("mymod.C", &["typing.Container"]);
        store.insert_class(facts).unwrap();
        let snapshot = store.class("mymod.C").unwrap();
        let want = Some("abc.ABCMeta".to_string());
        assert_eq!(snapshot.metaclass_fullname, want);
        let protocol = store.class("typing.Container").unwrap();
        assert_eq!(protocol.metaclass_fullname, want);
    }

    #[test]
    fn protocol_rule_refuses_a_store_without_the_abc_record() {
        let mut store = root_store();
        let mut protocol = with_bases("typing.Container", &["builtins.object"]);
        protocol.is_protocol = true;
        let err = store.insert_class(protocol).unwrap_err();
        assert_eq!(err, unknown_class("abc.ABCMeta"));
    }

    #[test]
    fn enum_metaclass_sets_is_enum() {
        let mut store = root_store();
        let meta = with_bases("enum.EnumMeta", &["builtins.type"]);
        store.insert_class(meta).unwrap();
        let mut facts = with_bases("colors.Color", &["builtins.object"]);
        facts.declared_metaclass = Some("enum.EnumMeta".to_string());
        store.insert_class(facts).unwrap();
        let snapshot = store.class("colors.Color").unwrap();
        assert!(snapshot.is_enum);
    }

    #[test]
    fn generic_enum_is_refused() {
        let mut store = root_store();
        let meta = with_bases("enum.EnumMeta", &["builtins.type"]);
        store.insert_class(meta).unwrap();
        let mut facts = with_bases("colors.Color", &["builtins.object"]);
        facts.declared_metaclass = Some("enum.EnumMeta".to_string());
        facts.type_vars.push(TypeVarFact {
            name: "T".to_string(),
            variance: 0,
            kind: 0,
            raw_id: 1,
            upper_bound: None,
            tuple_fallback: None,
        });
        let err = store.insert_class(facts).unwrap_err();
        let reason = "an enum class cannot be generic (semanal.py:4050)";
        assert_eq!(err, unsupported("colors.Color", reason));
    }

    #[test]
    fn protocol_members_skip_object_auxiliary_and_excluded_names() {
        let mut store = protocol_store();
        let facts = with_bases("mymod.C", &["typing.Container"]);
        store.insert_class(facts).unwrap();
        let snapshot = store.class("mymod.C").unwrap();
        let want = names(&["Nested", "__contains__"]);
        assert_eq!(snapshot.protocol_members, want);
    }

    #[test]
    fn abstract_member_makes_the_class_abstract() {
        let mut store = protocol_store();
        let facts = with_bases("mymod.C", &["typing.Container"]);
        store.insert_class(facts).unwrap();
        assert!(store.class("mymod.C").unwrap().is_abstract);
    }

    #[test]
    fn concrete_member_shadows_the_abstract_one() {
        let mut store = protocol_store();
        let mut facts = with_bases("mymod.C", &["typing.Container"]);
        let concrete = MemberFact::function();
        facts.members.insert("__contains__".to_string(), concrete);
        store.insert_class(facts).unwrap();
        assert!(!store.class("mymod.C").unwrap().is_abstract);
    }

    #[test]
    fn newtype_is_never_abstract() {
        let mut store = protocol_store();
        let mut facts = with_bases("mymod.C", &["typing.Container"]);
        facts.is_newtype = true;
        store.insert_class(facts).unwrap();
        assert!(!store.class("mymod.C").unwrap().is_abstract);
    }

    #[test]
    fn promotion_uses_the_table_and_the_module_symbol_table() {
        let mut store = root_store();
        store.insert_module(builtins_classes(&["float", "complex"]));
        let float = with_bases("builtins.float", &["builtins.object"]);
        store.insert_class(float).unwrap();
        let int = with_bases("builtins.int", &["builtins.object"]);
        store.insert_class(int).unwrap();
        let got = store.promote("builtins.int").unwrap();
        assert_eq!(got, vec![instance_type("builtins.float", &[])]);
        let got = store.promote("builtins.float").unwrap();
        assert_eq!(got, vec![instance_type("builtins.complex", &[])]);
        let got = store.promote("builtins.object").unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn promotion_table_matches_the_python_source() {
        assert_eq!(promotion_target("builtins.int"), Some("float"));
        assert_eq!(promotion_target("builtins.float"), Some("complex"));
        assert_eq!(promotion_target("builtins.bytearray"), Some("bytes"));
        assert_eq!(promotion_target("builtins.memoryview"), Some("bytes"));
        assert_eq!(promotion_target("builtins.str"), None);
    }

    #[test]
    fn explicit_promote_wins_over_the_table() {
        let mut store = root_store();
        let mut facts = with_bases("builtins.int", &["builtins.object"]);
        facts.promote.push(instance_type("mymod.Wide", &[]));
        store.insert_class(facts).unwrap();
        let got = store.promote("builtins.int").unwrap();
        assert_eq!(got, vec![instance_type("mymod.Wide", &[])]);
    }

    #[test]
    fn promotion_target_must_be_a_class_symbol() {
        let mut store = root_store();
        let mut module = builtins_classes(&["float"]);
        let fact = SymbolFact::of_kind("builtins", "float", SymbolKind::Var);
        module.symbols.insert("float".to_string(), fact);
        store.insert_module(module);
        let facts = with_bases("builtins.int", &["builtins.object"]);
        let err = store.insert_class(facts).unwrap_err();
        assert_eq!(err, unresolved_promotion("builtins.int", "float"));
    }

    #[test]
    fn promotion_needs_the_module_symbol_table() {
        let mut store = root_store();
        let facts = with_bases("builtins.int", &["builtins.object"]);
        let err = store.insert_class(facts).unwrap_err();
        assert_eq!(err, unknown_symbol("builtins", "float"));
    }

    #[test]
    fn member_maps_follow_the_node_kind_rule() {
        let mut store = root_store();
        let mut facts = with_bases("mymod.C", &["builtins.object"]);
        add_member(&mut facts, "method", MemberFact::function());
        add_member(&mut facts, "prop", MemberFact::decorator());
        add_member(&mut facts, "attr", MemberFact::var(true));
        let alias = MemberFact::of_kind(MemberKind::TypeAlias);
        add_member(&mut facts, "alias", alias);
        store.insert_class(facts).unwrap();
        let snapshot = store.class("mymod.C").unwrap();
        assert_eq!(snapshot.member_info.len(), 4);
        assert_eq!(snapshot.member_definers.len(), 3);
        let want = (0, "mymod.C".to_string());
        assert_eq!(snapshot.member_definers["method"], want);
        let want = (1, "mymod.C".to_string());
        assert_eq!(snapshot.member_definers["prop"], want);
        let want = (2, "mymod.C".to_string());
        assert_eq!(snapshot.member_definers["attr"], want);
        assert_eq!(snapshot.member_info["attr"], (false, true));
        assert_eq!(snapshot.member_info["alias"], (false, false));
        let member = store.member("mymod.C", "alias").unwrap().unwrap();
        assert_eq!(member.kind, MemberKind::TypeAlias);
        assert!(store.member("mymod.C", "missing").unwrap().is_none());
    }

    #[test]
    fn type_var_facts_fill_the_parallel_arrays() {
        let mut store = root_store();
        let mut facts = with_bases("mymod.Box", &["builtins.object"]);
        let bound = instance_type("builtins.object", &[]);
        facts.type_vars.push(TypeVarFact {
            name: "T".to_string(),
            variance: 1,
            kind: 0,
            raw_id: 1,
            upper_bound: Some(bound),
            tuple_fallback: None,
        });
        facts.type_vars.push(TypeVarFact {
            name: "P".to_string(),
            variance: 0,
            kind: 1,
            raw_id: 2,
            upper_bound: None,
            tuple_fallback: None,
        });
        store.insert_class(facts).unwrap();
        let snapshot = store.class("mymod.Box").unwrap();
        assert_eq!(snapshot.type_vars, names(&["T", "P"]));
        assert_eq!(snapshot.type_var_raw_ids, vec![1, 2]);
        let want = vec![("T".to_string(), 1, 0), ("P".to_string(), 0, 1)];
        assert_eq!(snapshot.type_vars_with_variance, want);
        assert!(snapshot.has_param_spec_type);
        assert!(!snapshot.has_type_var_tuple_type);
        assert_eq!(store.variance("mymod.Box", "T").unwrap(), Some(1));
        assert_eq!(store.variance("mymod.Box", "Q").unwrap(), None);
        let object = instance_type("builtins.object", &[]);
        let want = vec![Some(object), None];
        let bounds = store.type_var_upper_bounds("mymod.Box").unwrap();
        assert_eq!(bounds, want);
    }

    #[test]
    fn queries_decode_what_the_snapshot_stores() {
        let mut store = root_store();
        let mut facts = with_bases("mymod.Pair", &["builtins.object"]);
        facts.tuple_type = Some(instance_type("builtins.tuple", &[]));
        facts.type_var_tuple_prefix = Some(1);
        facts.type_var_tuple_suffix = Some(2);
        store.insert_class(facts).unwrap();
        let got = store.bases("mymod.Pair").unwrap();
        assert_eq!(got, vec![instance_type("builtins.object", &[])]);
        let got = store.tuple_type("mymod.Pair").unwrap();
        assert_eq!(got, Some(instance_type("builtins.tuple", &[])));
        assert_eq!(store.tuple_type("builtins.object").unwrap(), None);
        let snapshot = store.class("mymod.Pair").unwrap();
        assert_eq!(snapshot.type_var_tuple_prefix, Some(1));
        assert_eq!(snapshot.type_var_tuple_suffix, Some(2));
        let err = store.bases("mymod.Absent").unwrap_err();
        assert_eq!(err, unknown_class("mymod.Absent"));
    }

    #[test]
    fn fallback_to_any_is_inherited_along_the_mro() {
        let mut store = root_store();
        let mut base = with_bases("mymod.Base", &["builtins.object"]);
        base.fallback_to_any = true;
        store.insert_class(base).unwrap();
        let child = with_bases("mymod.Child", &["mymod.Base"]);
        store.insert_class(child).unwrap();
        assert!(store.class("mymod.Child").unwrap().fallback_to_any);
        assert!(!store.class("builtins.object").unwrap().fallback_to_any);
    }

    #[test]
    fn resolver_carries_the_classes_and_the_module_snapshots() {
        let mut store = root_store();
        store.insert_module(builtins_classes(&["float"]));
        let int = with_bases("builtins.int", &["builtins.object"]);
        store.insert_class(int).unwrap();
        let resolver = store.resolver();
        assert_eq!(resolver.len(), 3);
        assert!(resolver.get("builtins.int").is_some());
        let module = resolver.get_module("builtins").unwrap();
        assert_eq!(module.visible("float"), Some(true));
        assert_eq!(module.visible("missing"), None);
    }

    #[test]
    fn symbol_lookup_covers_module_class_and_function_scopes() {
        let mut store = root_store();
        store.insert_module(builtins_classes(&["float"]));
        let mut facts = with_bases("mymod.C", &["builtins.object"]);
        add_member(&mut facts, "method", MemberFact::function());
        store.insert_class(facts).unwrap();
        let scope = SymbolScope::Module("builtins".to_string());
        let got = store.lookup(&scope, "float").unwrap().unwrap();
        assert_eq!(got.fullname, "builtins.float");
        assert_eq!(got.kind, SymbolKind::Class);
        let scope = SymbolScope::Class("mymod.C".to_string());
        let got = store.lookup(&scope, "method").unwrap().unwrap();
        assert_eq!(got.fullname, "mymod.C.method");
        assert_eq!(got.kind, SymbolKind::Function);
        assert!(store.lookup(&scope, "float").unwrap().is_none());
        let scope = SymbolScope::Function {
            module: "builtins".to_string(),
            function: "f".to_string(),
            class: Some("mymod.C".to_string()),
        };
        let got = store.lookup(&scope, "method").unwrap().unwrap();
        assert_eq!(got.scope, scope);
        let got = store.lookup(&scope, "float").unwrap().unwrap();
        assert_eq!(got.fullname, "builtins.float");
        assert!(store.lookup(&scope, "local").unwrap().is_none());
        let scope = SymbolScope::Module("mymod".to_string());
        let err = store.lookup(&scope, "float").unwrap_err();
        assert_eq!(err, unknown_module("mymod"));
    }

    #[test]
    fn module_snapshot_projects_visibility_and_submodules() {
        let mut module = ModuleFacts {
            fullname: "pkg".to_string(),
            symbols: BTreeMap::new(),
        };
        let public = SymbolFact::class("pkg", "pub");
        module.symbols.insert("pub".to_string(), public);
        let mut hidden = SymbolFact::class("other", "hidden");
        hidden.module_hidden = true;
        module.symbols.insert("hidden".to_string(), hidden);
        let sub = SymbolFact::of_kind("pkg", "sub", SymbolKind::Module);
        module.symbols.insert("sub".to_string(), sub);
        let snapshot = module.snapshot();
        assert_eq!(snapshot.visible("pub"), Some(true));
        assert_eq!(snapshot.visible("hidden"), Some(false));
        assert_eq!(snapshot.visible("absent"), None);
        assert_eq!(snapshot.module_fullname("sub"), Some("pkg.sub"));
        assert_eq!(snapshot.module_fullname("pub"), None);
        assert_eq!(snapshot.module_fullname("hidden"), None);
        assert!(module.resolve("hidden").is_none());
    }

    #[test]
    fn disjoint_base_changes_no_snapshot_field() {
        let mut plain_store = root_store();
        let mut flagged_store = root_store();
        let plain = with_bases("mymod.C", &["builtins.object"]);
        let mut flagged = with_bases("mymod.C", &["builtins.object"]);
        flagged.disjoint_base = true;
        plain_store.insert_class(plain).unwrap();
        flagged_store.insert_class(flagged).unwrap();
        let plain_record = plain_store.class("mymod.C");
        let flagged_record = flagged_store.class("mymod.C");
        assert_eq!(plain_record, flagged_record);
        assert!(flagged_store.facts("mymod.C").unwrap().disjoint_base);
        assert!(!plain_store.facts("mymod.C").unwrap().disjoint_base);
    }

    #[test]
    fn name_falls_back_to_the_last_dotted_segment() {
        assert_eq!(short_name("builtins.int"), "int");
        assert_eq!(short_name("int"), "int");
        assert_eq!(module_of("builtins.int"), "builtins");
        assert_eq!(module_of("int"), "");
        assert_eq!(qualified("builtins", "int"), "builtins.int");
        assert_eq!(qualified("", "int"), "int");
        let mut store = root_store();
        let mut facts = with_bases("mymod.C", &["builtins.object"]);
        facts.name = "renamed".to_string();
        store.insert_class(facts).unwrap();
        assert_eq!(store.class("mymod.C").unwrap().name, "renamed");
    }

    #[test]
    fn empty_fullname_is_refused() {
        let mut store = root_store();
        let err = store.insert_class(ClassFacts::new("")).unwrap_err();
        assert_eq!(err, unsupported("", "a class record needs a fullname"));
    }

    #[test]
    fn member_kind_node_kinds_match_the_snapshot_vocabulary() {
        assert_eq!(MemberKind::Function.node_kind(), 0);
        assert_eq!(MemberKind::Decorator.node_kind(), 1);
        assert_eq!(MemberKind::Var.node_kind(), 2);
        assert_eq!(MemberKind::TypeAlias.node_kind(), -1);
        assert_eq!(MemberKind::TypeVarLike.node_kind(), -1);
        assert_eq!(MemberKind::Module.node_kind(), -1);
        assert_eq!(MemberKind::Other.node_kind(), -1);
        assert!(MemberKind::TypeAlias.is_auxiliary());
        assert!(MemberKind::TypeVarLike.is_auxiliary());
        assert!(MemberKind::Module.is_auxiliary());
        assert!(!MemberKind::Other.is_auxiliary());
        assert!(!MemberKind::Function.is_auxiliary());
    }

    #[test]
    fn reinserting_a_class_replaces_its_record() {
        let mut store = root_store();
        let facts = with_bases("mymod.C", &["builtins.object"]);
        store.insert_class(facts).unwrap();
        assert_eq!(store.len(), 3);
        let mut grown = with_bases("mymod.C", &["builtins.object"]);
        add_member(&mut grown, "method", MemberFact::function());
        store.insert_class(grown).unwrap();
        assert_eq!(store.len(), 3);
        assert!(store.member("mymod.C", "method").unwrap().is_some());
    }
}
