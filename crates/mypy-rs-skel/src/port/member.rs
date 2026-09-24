//! Standalone driver glue: member.
//!
//! Drive the kernel member-access api over skeleton-owned records.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::member`. It owns no checking logic of its
//! own: it adapts skeleton records (`crate::model`, `crate::fixtures`) to
//! the kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `member` lane for this wave. Add
//! `#[cfg(test)]` unit tests here.
//!
//! # What this adapter adds
//!
//! Two things, and nothing else:
//!
//! 1. *Record bridging.* The kernel answers questions about a class
//!    fullname and a `TypeResolver`; the driver holds `crate::model::
//!    ClassModel`s and one resolver built from `crate::fixtures::Fixtures`
//!    plus the per-class snapshots `crate::model::snapshot` produces. The
//!    `model_*` methods are that bridge.
//! 2. *Deferral classification.* Every kernel member query returns
//!    `Option`, where `None` means "the snapshots do not carry enough to
//!    decide". A silent `None` would become a silently wrong check, so
//!    each method converts it into a `CheckError::Internal` naming the
//!    operation and the source line, following `check.rs`'s
//!    `require_decidable` precedent: a kernel deferral on a file the
//!    subset covers is the fixtures' fault, not the user's.
//!
//! No method here decides a mypy question itself. If a method ever grows
//! a branch on a type shape or a member name, that logic belongs in
//! `type_kernel::standalone::member` and is being duplicated here.
//!
//! # Not answerable from the records exposed so far
//!
//! `check.rs::type_attr` distinguishes a class variable from an instance
//! attribute by matching `crate::model::Member::ClassVar` against
//! `Member::InstanceVar`. The kernel records the same distinction, as the
//! `(implicit, has_explicit_value)` pair in `TypeInfoSnapshot::
//! member_info`, but the only operation over it that
//! `type_kernel::standalone::member` exposes today is
//! `defined_in_superclass`, which by design skips the class's own MRO
//! entry. Asking "is `name` a class attribute *of this class*" therefore
//! has no kernel operation yet; wave 2 needs one before `type_attr` can
//! stop reading `crate::model::Member` directly.

pub use type_kernel::standalone::member::{
    DescriptorProtocol, MemberAccessKind, Type, TypeResolver,
};

use type_kernel::standalone::member as api;

use crate::check::CheckError;
use crate::model::ClassModel;

/// A kernel member query's outcome: the answer, or the loud rejection of
/// a deferral.
pub type MemberResult<T> = Result<T, CheckError>;

/// The member-access questions the driver asks, over the kernel's
/// standalone member API.
///
/// Borrows the resolver for its whole life, so a caller that mutates the
/// snapshots mid-class (as `check.rs::refresh` does when a member is
/// registered) must drop the reader first. That is deliberate: a reader
/// that outlived a snapshot refresh would answer from stale facts.
pub struct MemberReader<'a> {
    resolver: &'a TypeResolver,
    path: &'a str,
}

impl<'a> MemberReader<'a> {
    /// A reader over `resolver`, reporting failures against `path`.
    pub fn new(resolver: &'a TypeResolver, path: &'a str) -> Self {
        Self { resolver, path }
    }

    /// Turn the kernel's `None` deferral into an internal error naming
    /// `op`. Uniform on purpose: deciding which deferrals are the user's
    /// fault rather than the fixtures' is a policy call this adapter does
    /// not own.
    fn decided<T>(&self, verdict: Option<T>, op: &str, line: usize) -> MemberResult<T> {
        match verdict {
            Some(v) => Ok(v),
            None => Err(CheckError::Internal(format!(
                "{}:{line}: skeleton internal error: the kernel deferred the {op} member \
                 query; the fixture closure no longer covers the corpus",
                self.path
            ))),
        }
    }

    /// Which `mypy/checkmember.py::_analyze_member_access` branch a
    /// receiver dispatches to. The driver uses this to reject a receiver
    /// shape the subset does not model instead of guessing a branch.
    pub fn dispatch_kind(&self, recv: &Type, line: usize) -> MemberResult<MemberAccessKind> {
        let kind = api::classify_member_access(recv, self.resolver);
        self.decided(kind, "classify_member_access", line)
    }

    /// `mypy/nodes.py::TypeInfo.has_readable_member`: does `name` exist
    /// anywhere in `class`'s MRO? This is the question behind mypy's
    /// `"X" has no attribute "y"`, so a `false` is a real answer and only
    /// a missing snapshot defers.
    pub fn has_member(&self, class: &str, name: &str, line: usize) -> MemberResult<bool> {
        let found = api::has_readable_member(class, name, self.resolver);
        self.decided(found, "has_readable_member", line)
    }

    /// The same existence query for a corpus class the driver owns a
    /// model for.
    pub fn model_has_member(
        &self,
        model: &ClassModel,
        name: &str,
        line: usize,
    ) -> MemberResult<bool> {
        self.has_member(&model.fullname, name, line)
    }

    /// `mypy/checkmember.py::defined_in_superclass`: is `name` a
    /// class-level attribute with an explicit value in some *base* of
    /// `class`? The class's own entry is skipped, matching mypy, so a
    /// member `class` defines itself answers `false`.
    pub fn class_attr_in_base(&self, class: &str, name: &str, line: usize) -> MemberResult<bool> {
        let found = api::defined_in_superclass(class, name, self.resolver);
        self.decided(found, "defined_in_superclass", line)
    }

    /// The `__get__` / `__set__` presence behind
    /// `mypy/checkmember.py::analyze_descriptor_access`.
    pub fn descriptor_protocol(
        &self,
        recv: &Type,
        line: usize,
    ) -> MemberResult<DescriptorProtocol> {
        let flags = api::descriptor_has_get_set(recv, self.resolver);
        self.decided(flags, "descriptor_has_get_set", line)
    }

    /// `mypy/subtypes.py::is_descriptor`: does this type implement the
    /// descriptor protocol?
    pub fn is_descriptor(&self, recv: &Type, line: usize) -> MemberResult<bool> {
        let verdict = api::is_descriptor(recv, self.resolver);
        self.decided(verdict, "is_descriptor", line)
    }

    /// `mypy/checkmember.py::has_operator`: does `recv` implement the
    /// operator method `op`? `strict_optional` is the driver's
    /// `Options.strict_optional`, which decides whether a `None` arm of a
    /// union is a relevant item.
    pub fn has_operator_method(
        &self,
        recv: &Type,
        op: &str,
        strict_optional: bool,
        line: usize,
    ) -> MemberResult<bool> {
        let verdict = api::has_operator(recv, op, strict_optional, self.resolver);
        self.decided(verdict, "has_operator", line)
    }

    /// `mypy/checkmember.py::meta_has_operator`: the same question on the
    /// receiver's metaclass, which is where a class-object operand's
    /// operator methods live.
    pub fn has_meta_operator_method(
        &self,
        recv: &Type,
        op: &str,
        line: usize,
    ) -> MemberResult<bool> {
        let verdict = api::meta_has_operator(recv, op, self.resolver);
        self.decided(verdict, "meta_has_operator", line)
    }

    /// `mypy/typeops.py::custom_special_method`: does `recv` override
    /// `name` with a non-`builtins` definition?
    pub fn custom_special_method(
        &self,
        recv: &Type,
        name: &str,
        check_all: bool,
        line: usize,
    ) -> MemberResult<bool> {
        let verdict = api::custom_special_method(recv, name, check_all, self.resolver);
        self.decided(verdict, "custom_special_method", line)
    }

    /// `mypy/checkmember.py::bind_self_fast`: the bound form of a method
    /// signature. Defers on a non-callable and on a signature still
    /// carrying an erasure placeholder.
    pub fn bind_self(&self, signature: &Type, line: usize) -> MemberResult<Type> {
        let bound = api::bind_self_fast(signature);
        self.decided(bound, "bind_self_fast", line)
    }

    /// `mypy/checkmember.py::instance_fallback`: the `Instance` a member
    /// access on `recv` actually resolves against.
    pub fn fallback_instance(&self, recv: &Type, line: usize) -> MemberResult<Type> {
        let fb = api::instance_fallback(recv);
        self.decided(fb, "instance_fallback", line)
    }

    /// `mypy/types.py::get_proper_type`. Defers on a `TypeAliasType`,
    /// whose expansion target the standalone path has no alias store for
    /// yet; the driver must reject that shape rather than read through it.
    pub fn proper_type<'t>(&self, typ: &'t Type, line: usize) -> MemberResult<&'t Type> {
        self.decided(api::get_proper_type(typ), "get_proper_type", line)
    }

    /// `mypy/types.py::CallableType.is_type_obj`: is this callable a class
    /// object? Infallible, so it needs no line.
    pub fn is_type_object(&self, fallback: &Type, ret_type: &Type) -> bool {
        api::is_type_obj(fallback, ret_type, self.resolver)
    }

    /// `mypy/checkmember.py::analyze_none_member_access`'s `__bool__` arm:
    /// the type of `None.__bool__`. Infallible.
    pub fn none_bool_method(&self) -> Type {
        api::none_bool_method_type()
    }

    /// `mypy/types.py::TypeType.make_normalized`: the class-object type of
    /// `item`, distributing over a union. Infallible.
    pub fn class_object_of(&self, item: &Type) -> Type {
        api::make_type_type_normalized(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use type_kernel::standalone::member::TypeInfoSnapshot;

    const ARG_POS: i64 = 0;

    fn inst(type_ref: &str) -> Type {
        Type::Instance {
            type_ref: type_ref.to_string(),
            args: vec![],
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn class(fullname: &str, mro: &[&str], members: &[(&str, (bool, bool))]) -> TypeInfoSnapshot {
        let mut s = TypeInfoSnapshot {
            fullname: fullname.to_string(),
            name: fullname.to_string(),
            ..Default::default()
        };
        for entry in mro {
            let name = entry.to_string();
            s.mro.push(name.clone());
            s.has_base.insert(name);
        }
        for (name, flags) in members {
            s.member_info.insert(name.to_string(), *flags);
        }
        s
    }

    /// Record `name` as a method of the snapshot, defined by `definer`.
    fn with_definer(mut s: TypeInfoSnapshot, name: &str, definer: &str) -> TypeInfoSnapshot {
        let entry = (0i64, definer.to_string());
        s.member_definers.insert(name.to_string(), entry);
        s
    }

    fn resolver_with(extra: Vec<TypeInfoSnapshot>) -> TypeResolver {
        let mut r = TypeResolver::new();
        let object = class("builtins.object", &["builtins.object"], &[]);
        r.insert("builtins.object".to_string(), object);
        for s in extra {
            let key = s.fullname.clone();
            r.insert(key, s);
        }
        r
    }

    fn model(fullname: &str, mro: Vec<String>) -> ClassModel {
        ClassModel {
            fullname: fullname.to_string(),
            tvars: vec![],
            bases: vec![],
            mro,
            members: BTreeMap::new(),
        }
    }

    fn method_sig() -> Type {
        Type::CallableType {
            fallback: Box::new(inst("builtins.function")),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: false,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs: false,
            from_type_type: false,
            arg_types: vec![inst("C")],
            arg_kinds: vec![ARG_POS],
            arg_names: vec![Some("self".to_string())],
            ret_type: Box::new(inst("builtins.int")),
            name: Some("method".to_string()),
            variables: vec![],
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    /// The message of an `Internal` failure; anything else is a bug in the
    /// adapter, since a kernel deferral is never the user's fault.
    fn internal_message(result: MemberResult<bool>) -> String {
        match result {
            Ok(v) => panic!("expected a deferral, got {v}"),
            Err(CheckError::Internal(m)) => m,
            Err(CheckError::Input(m)) => panic!("expected Internal, got Input: {m}"),
        }
    }

    /// Unwrap an expected answer; a deferral reaching this helper is a
    /// test failure. `CheckError` derives only `Debug`, so a `Result`
    /// against it is not comparable and the tests assert on the unwrapped
    /// value instead.
    fn answer<T: std::fmt::Debug>(result: MemberResult<T>) -> T {
        match result {
            Ok(v) => v,
            Err(e) => panic!("expected an answer, got {e:?}"),
        }
    }

    #[test]
    fn dispatch_kind_answers_for_a_covered_instance() {
        let r = resolver_with(vec![class("C", &["C", "builtins.object"], &[])]);
        let reader = MemberReader::new(&r, "mod.py");
        let kind = answer(reader.dispatch_kind(&inst("C"), 1));
        assert_eq!(kind, MemberAccessKind::Instance);
    }

    #[test]
    fn dispatch_kind_rejects_a_deferral_loudly() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let alias = Type::TypeAliasType {
            args: vec![],
            type_ref: "mod.Alias".to_string(),
            is_recursive: false,
        };
        let message = match reader.dispatch_kind(&alias, 7) {
            Ok(kind) => panic!("expected a deferral, got {kind:?}"),
            Err(CheckError::Internal(m)) => m,
            Err(CheckError::Input(m)) => panic!("expected Internal, got Input: {m}"),
        };
        assert!(message.contains("mod.py:7"), "names the site");
        assert!(
            message.contains("classify_member_access"),
            "must name the operation: {message}"
        );
    }

    #[test]
    fn has_member_answers_from_the_mro() {
        let value = [("value", (false, true))];
        let b = class("B", &["B", "builtins.object"], &value);
        let c = class("C", &["C", "B", "builtins.object"], &[]);
        let r = resolver_with(vec![b, c]);
        let reader = MemberReader::new(&r, "mod.py");
        assert!(answer(reader.has_member("C", "value", 1)));
    }

    /// Rejection: an instance with no such member is a real `false`, not a
    /// deferral, because that is the answer mypy's "has no attribute"
    /// diagnostic is built from.
    #[test]
    fn has_member_is_false_for_an_absent_member() {
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![c]);
        let reader = MemberReader::new(&r, "mod.py");
        assert!(!answer(reader.has_member("C", "missing", 3)));
    }

    #[test]
    fn has_member_defers_when_the_class_is_unsnapshotted() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let message = internal_message(reader.has_member("C", "value", 4));
        assert!(message.contains("has_readable_member"), "{message}");
    }

    #[test]
    fn model_has_member_bridges_the_class_model() {
        let c = class(
            "mod.C",
            &["mod.C", "builtins.object"],
            &[("v", (false, true))],
        );
        let r = resolver_with(vec![c]);
        let reader = MemberReader::new(&r, "mod.py");
        let m = model("mod.C", vec!["mod.C".to_string()]);
        assert!(answer(reader.model_has_member(&m, "v", 1)));
        assert!(!answer(reader.model_has_member(&m, "absent", 1)));
    }

    #[test]
    fn class_attr_in_base_finds_a_class_attribute() {
        let shape = [("shape", (false, true))];
        let b = class("B", &["B", "builtins.object"], &shape);
        let c = class("C", &["C", "B", "builtins.object"], &[]);
        let r = resolver_with(vec![b, c]);
        let reader = MemberReader::new(&r, "mod.py");
        assert!(answer(reader.class_attr_in_base("C", "shape", 1)));
    }

    /// Rejection: the class-attribute-versus-instance-attribute mismatch.
    /// `shape` is readable, but as an implicit instance attribute it is not
    /// a base-class attribute, and the two answers must not collapse.
    #[test]
    fn class_attr_in_base_rejects_an_instance_attribute() {
        let shape = [("shape", (true, false))];
        let b = class("B", &["B", "builtins.object"], &shape);
        let c = class("C", &["C", "B", "builtins.object"], &[]);
        let r = resolver_with(vec![b, c]);
        let reader = MemberReader::new(&r, "mod.py");
        assert!(answer(reader.has_member("C", "shape", 1)));
        assert!(!answer(reader.class_attr_in_base("C", "shape", 1)));
    }

    #[test]
    fn descriptor_protocol_reports_get_without_set() {
        let get = [("__get__", (false, false))];
        let d = class("D", &["D", "builtins.object"], &get);
        let r = resolver_with(vec![d]);
        let reader = MemberReader::new(&r, "mod.py");
        let expected = DescriptorProtocol {
            has_get: true,
            has_set: false,
        };
        let flags = answer(reader.descriptor_protocol(&inst("D"), 1));
        assert_eq!(flags, expected);
        assert!(answer(reader.is_descriptor(&inst("D"), 1)));
    }

    #[test]
    fn descriptor_protocol_rejects_a_plain_instance() {
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![c]);
        let reader = MemberReader::new(&r, "mod.py");
        let expected = DescriptorProtocol {
            has_get: false,
            has_set: false,
        };
        let flags = answer(reader.descriptor_protocol(&inst("C"), 1));
        assert_eq!(flags, expected);
        assert!(!answer(reader.is_descriptor(&inst("C"), 1)));
    }

    #[test]
    fn operator_method_queries_read_the_snapshots() {
        let add = [("__add__", (false, false))];
        let c = class("C", &["C", "builtins.object"], &add);
        let r = resolver_with(vec![c]);
        let reader = MemberReader::new(&r, "mod.py");
        let recv = inst("C");
        let found = answer(reader.has_operator_method(&recv, "__add__", true, 1));
        assert!(found);
        let absent = answer(reader.has_operator_method(&recv, "__sub__", true, 1));
        assert!(!absent);
    }

    #[test]
    fn operator_method_queries_defer_without_the_class() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let recv = inst("C");
        let verdict = reader.has_operator_method(&recv, "__add__", true, 2);
        let message = match verdict {
            Ok(v) => panic!("expected a deferral, got {v}"),
            Err(CheckError::Internal(m)) => m,
            Err(CheckError::Input(m)) => panic!("expected Internal, got Input: {m}"),
        };
        assert!(message.contains("has_operator"), "{message}");
    }

    #[test]
    fn meta_operator_query_defaults_to_builtins_type() {
        let meta = class("builtins.type", &["builtins.type", "builtins.object"], &[]);
        let c = class("C", &["C", "builtins.object"], &[]);
        let r = resolver_with(vec![meta, c]);
        let reader = MemberReader::new(&r, "mod.py");
        let recv = inst("C");
        let verdict = answer(reader.has_meta_operator_method(&recv, "__call__", 1));
        assert!(!verdict);
    }

    #[test]
    fn custom_special_method_separates_user_from_builtin() {
        let c = with_definer(class("C", &["C", "builtins.object"], &[]), "__eq__", "C");
        let d = with_definer(
            class("D", &["D", "builtins.object"], &[]),
            "__eq__",
            "builtins.object",
        );
        let r = resolver_with(vec![c, d]);
        let reader = MemberReader::new(&r, "mod.py");
        let own = inst("C");
        let custom = answer(reader.custom_special_method(&own, "__eq__", false, 1));
        assert!(custom);
        let other = inst("D");
        let plain = answer(reader.custom_special_method(&other, "__eq__", false, 1));
        assert!(!plain);
    }

    #[test]
    fn bind_self_strips_the_self_argument() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let bound = reader.bind_self(&method_sig(), 1);
        let Ok(bound) = bound else {
            panic!("a plain method must bind");
        };
        match bound {
            Type::CallableType {
                arg_types,
                is_bound,
                ..
            } => {
                assert!(arg_types.is_empty());
                assert!(is_bound);
            }
            other => panic!("expected a bound callable, got {other:?}"),
        }
    }

    /// Rejection: a non-callable has nothing to bind, and the adapter must
    /// say so rather than pass the receiver through.
    #[test]
    fn bind_self_rejects_a_non_callable() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let bound = reader.bind_self(&inst("C"), 5);
        let message = match bound {
            Ok(t) => panic!("expected a deferral, got {t:?}"),
            Err(CheckError::Internal(m)) => m,
            Err(CheckError::Input(m)) => panic!("expected Internal, got Input: {m}"),
        };
        assert!(message.contains("mod.py:5"), "{message}");
        assert!(message.contains("bind_self_fast"), "{message}");
    }

    #[test]
    fn fallback_instance_resolves_a_tuple_and_a_singleton() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let t = Type::TupleType {
            partial_fallback: Box::new(inst("builtins.tuple")),
            items: vec![inst("builtins.int")],
            implicit: false,
        };
        let tuple_fb = answer(reader.fallback_instance(&t, 1));
        assert_eq!(tuple_fb, inst("builtins.tuple"));
        let none_fb = answer(reader.fallback_instance(&Type::NoneType, 1));
        assert_eq!(none_fb, inst("builtins.object"));
    }

    #[test]
    fn proper_type_passes_through_and_rejects_an_alias() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let t = inst("C");
        let proper = answer(reader.proper_type(&t, 1));
        assert_eq!(proper, &t);
        let alias = Type::TypeAliasType {
            args: vec![],
            type_ref: "mod.Alias".to_string(),
            is_recursive: false,
        };
        let verdict = reader.proper_type(&alias, 6);
        let message = match verdict {
            Ok(_) => panic!("expected a deferral"),
            Err(CheckError::Internal(m)) => m,
            Err(CheckError::Input(m)) => panic!("expected Internal, got Input: {m}"),
        };
        assert!(message.contains("get_proper_type"), "{message}");
    }

    #[test]
    fn infallible_queries_need_no_resolver_coverage() {
        let r = resolver_with(vec![]);
        let reader = MemberReader::new(&r, "mod.py");
        let meta = inst("builtins.type");
        let func = inst("builtins.function");
        let c = inst("C");
        assert!(reader.is_type_object(&meta, &c));
        assert!(!reader.is_type_object(&func, &c));
        let wrapped = reader.class_object_of(&c);
        let expected = Type::TypeType {
            item: Box::new(inst("C")),
            is_type_form: false,
        };
        assert_eq!(wrapped, expected);
        let none_bool = reader.none_bool_method();
        assert!(matches!(none_bool, Type::CallableType { .. }));
    }
}
