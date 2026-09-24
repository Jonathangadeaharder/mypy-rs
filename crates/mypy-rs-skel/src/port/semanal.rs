//! Standalone driver glue: semanal.
//!
//! The skeleton's semantic passes on top of the kernel semanal api.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::semanal`. It owns no checking logic of its
//! own: it adapts skeleton records (`crate::model`, `crate::fixtures`) to
//! the kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `semanal` lane for this wave. Add
//! `#[cfg(test)]` unit tests here.
//!
//! # What this increment adapts
//!
//! Four driver-facing operations, each a skeleton record in and a
//! skeleton answer out, with the decision left to the kernel:
//!
//! * [`class_member_owner`] resolves a class member through a
//!   [`ClassModel`] record, owning the record-to-snapshot refresh the
//!   kernel lookup needs.
//! * [`resolve_import_from`] resolves a whole `from M import a, b` clause
//!   through the kernel's module snapshots and rejects loudly on the
//!   first name the records cannot decide (wave-1 rule 6).
//! * [`rewrite_signature_any`] applies mypy's two `Any` rewrites to every
//!   slot of a [`Sig`] record.
//! * [`configure_class_bases`] validates every base of a [`ClassModel`],
//!   owning the record-to-`Instance` rebuild and the `is_newtype` read.
//!
//! The kernel's `LookupOutcome::Deferred` never reaches the driver as a
//! silent skip: each adapter either proves it unreachable or turns it into
//! an `Err` naming the record or the clause the fixtures do not cover,
//! which is the skeleton's exit-3 internal-error channel.

use type_kernel::standalone::semanal::{
    self, BaseClassOptions, BaseConfiguration, LookupOutcome, Type, TypeResolver,
};

use crate::model::{self, ClassModel, Sig};

/// Where a class member is defined, in skeleton terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberOwner {
    /// The fullname of the MRO entry that defines the member, which is
    /// what mypy's override and attribute checks compare against.
    DefinedIn(String),
    /// No MRO entry of the class carries the name.
    Absent,
}

/// `mypy.semanal.SemanticAnalyzer.lookup_qualified` for a class member,
/// driven from a skeleton [`ClassModel`] record.
///
/// The adapter owns the record-to-resolver step: it refreshes `class`'s
/// kernel snapshot first, so the answer always reflects the record passed
/// in rather than whatever the resolver last held for that fullname.
/// `Driver::register_member` mutates a model after its first refresh, and
/// a stale snapshot would answer `Absent` for a member the record plainly
/// has. The lookup itself is `semanal::lookup_typeinfo_member`.
///
/// `Err` is the internal-error channel: either the record failed to
/// snapshot, or the resolver lost it between the refresh and the lookup.
pub fn class_member_owner(
    resolver: &mut TypeResolver,
    class: &ClassModel,
    name: &str,
) -> Result<MemberOwner, String> {
    model::refresh_snapshot(class, resolver)?;
    match semanal::lookup_typeinfo_member(resolver, &class.fullname, name) {
        LookupOutcome::Resolved(defining) => Ok(MemberOwner::DefinedIn(defining)),
        LookupOutcome::NotFound => Ok(MemberOwner::Absent),
        LookupOutcome::Deferred => Err(format!(
            "the resolver lost the snapshot for {} while looking up `{name}`",
            class.fullname
        )),
    }
}

/// Where one `from M import name` symbol resolves, in skeleton terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolOwner {
    /// The fullname of the module whose namespace holds the name.
    InModule(String),
    /// The name is `module_hidden`, which mypy reports as undefined.
    Hidden,
}

/// `mypy.semanal.SemanticAnalyzer.visit_import_from` name resolution,
/// driven from the kernel's module snapshots.
///
/// Resolves every name of one `from {module_fullname} import ...` clause
/// through `semanal::lookup_module_chain` and answers in source order. A
/// name the records cannot decide is a hard `Err` naming the clause, never
/// a silent skip: the standalone path has no Python symbol table to fall
/// back to, so an undecided name means the fixtures do not cover it.
pub fn resolve_import_from(
    resolver: &TypeResolver,
    module_fullname: &str,
    names: &[&str],
) -> Result<Vec<SymbolOwner>, String> {
    let mut owners = Vec::with_capacity(names.len());
    for name in names {
        let dotted = format!("{module_fullname}.{name}");
        match semanal::lookup_module_chain(resolver, module_fullname, &dotted) {
            LookupOutcome::Resolved(full) => owners.push(SymbolOwner::InModule(full)),
            LookupOutcome::NotFound => owners.push(SymbolOwner::Hidden),
            LookupOutcome::Deferred => {
                return Err(format!(
                    "`from {module_fullname} import {name}` is outside the \
                     recorded module symbol tables"
                ))
            }
        }
    }
    Ok(owners)
}

/// Which of mypy's two `Any` rewrites to apply to a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyRewrite {
    /// `mypy.semanal.make_any_non_explicit`: an inlined alias target is no
    /// longer an explicit `Any`.
    NonExplicit,
    /// `mypy.semanal.make_any_non_unimported`: an `Any` that came from an
    /// unimported type is reported once, then normalized.
    NonUnimported,
}

/// The `Any` rewrite of `mypy.semanal.check_and_set_up_type_alias`
/// (semanal.py:5463-5468) applied to a skeleton [`Sig`] record.
///
/// Maps `semanal::make_any_non_explicit` / `make_any_non_unimported` over
/// every parameter type and the return type, so the driver gets a
/// rewritten `Sig` back instead of a bare `Type` it would have to
/// re-thread through `model::subst_sig` itself.
pub fn rewrite_signature_any(sig: &Sig, rewrite: AnyRewrite) -> Sig {
    let apply = |t: &Type| match rewrite {
        AnyRewrite::NonExplicit => semanal::make_any_non_explicit(t.clone()),
        AnyRewrite::NonUnimported => semanal::make_any_non_unimported(t.clone()),
    };
    Sig {
        params: sig
            .params
            .iter()
            .map(|(name, ty)| (name.clone(), apply(ty)))
            .collect(),
        ret: apply(&sig.ret),
    }
}

/// `mypy.semanal.SemanticAnalyzer.configure_base_classes` per-base
/// validation (semanal.py:3348-3381), driven from a skeleton
/// [`ClassModel`].
///
/// The adapter owns the record-to-type step: it rebuilds each base's
/// `Instance` the same way `model::snapshot` does, and reads
/// `is_newtype` from that base's snapshot, which is exactly the
/// `base.type.is_newtype` mypy tests before failing 'Cannot subclass
/// "NewType"'. The decision per base is `semanal::configure_base_class`;
/// nothing here classifies anything itself.
///
/// `Err` is the internal-error channel, taken when a base has no snapshot
/// (a fixture gap that cannot be proved harmless) or when the kernel
/// defers on an alias the records cannot expand. Neither is skipped: a
/// silently dropped base would change the hierarchy.
pub fn configure_class_bases(
    resolver: &TypeResolver,
    class: &ClassModel,
    opts: &BaseClassOptions,
) -> Result<Vec<BaseConfiguration>, String> {
    let mut configurations = Vec::with_capacity(class.bases.len());
    for (fullname, args) in &class.bases {
        let base = model::instance(fullname, args.clone());
        let snapshot = match resolver.get(fullname) {
            Some(snapshot) => snapshot,
            None => return Err(format!("no snapshot for the base {fullname}")),
        };
        let decided = semanal::configure_base_class(&base, snapshot.is_newtype, opts);
        let Some(configuration) = decided else {
            return Err(format!("the base {fullname} holds an unexpandable alias"));
        };
        configurations.push(configuration);
    }
    Ok(configurations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{object_type, Member};
    use std::collections::{BTreeMap, HashMap};

    use type_kernel::standalone::semanal::{BaseKind, TypeInfoSnapshot};

    /// `TypeOfAny.explicit` (mypy/types.py:213-239).
    const EXPLICIT: i64 = 2;
    /// `TypeOfAny.from_unimported_type` (mypy/types.py:213-239).
    const FROM_UNIMPORTED: i64 = 3;
    /// `TypeOfAny.from_error` (mypy/types.py:213-239).
    const FROM_ERROR: i64 = 5;
    /// `TypeOfAny.special_form` (mypy/types.py:213-239).
    const SPECIAL_FORM: i64 = 6;

    fn any(type_of_any: i64) -> Type {
        Type::AnyType {
            type_of_any,
            source_any: None,
            missing_import_name: None,
        }
    }

    fn any_kind(t: &Type) -> i64 {
        match t {
            Type::AnyType { type_of_any, .. } => *type_of_any,
            other => panic!("expected an AnyType, got {other:?}"),
        }
    }

    fn class_model(fullname: &str, mro: &[&str], members: &[&str]) -> ClassModel {
        let mut members_map = BTreeMap::new();
        for name in members {
            members_map.insert(name.to_string(), Member::ClassVar(object_type()));
        }
        ClassModel {
            fullname: fullname.to_string(),
            tvars: Vec::new(),
            bases: Vec::new(),
            mro: mro.iter().map(|m| m.to_string()).collect(),
            members: members_map,
        }
    }

    fn subclass_of(fullname: &str, bases: &[&str]) -> ClassModel {
        let mut built = class_model(fullname, &[fullname], &[]);
        for base in bases {
            built.bases.push((base.to_string(), Vec::new()));
        }
        built
    }

    fn resolver_with_module(fullname: &str, names: &[&str]) -> TypeResolver {
        let mut symbols = HashMap::new();
        for name in names {
            symbols.insert(name.to_string(), (false, None));
        }
        let snapshot = semanal::ModuleSnapshot { symbols };
        let mut resolver = TypeResolver::new();
        resolver.insert_module(fullname.to_string(), snapshot);
        resolver
    }

    fn sig_with(param: Type, ret: Type) -> Sig {
        Sig {
            params: vec![("x".to_string(), param)],
            ret,
        }
    }

    #[test]
    fn class_member_owner_names_the_defining_class() {
        let mut resolver = TypeResolver::new();
        let base = class_model("mod.Base", &["mod.Base"], &["shape"]);
        model::refresh_snapshot(&base, &mut resolver).unwrap();
        let sub = class_model("mod.Sub", &["mod.Sub", "mod.Base"], &[]);
        let owner = class_member_owner(&mut resolver, &sub, "shape");
        let in_base = MemberOwner::DefinedIn("mod.Base".to_string());
        assert_eq!(owner.unwrap(), in_base);
    }

    #[test]
    fn class_member_owner_reports_a_member_no_mro_entry_has() {
        let mut resolver = TypeResolver::new();
        let sub = class_model("mod.Sub", &["mod.Sub"], &[]);
        let owner = class_member_owner(&mut resolver, &sub, "shape");
        assert_eq!(owner.unwrap(), MemberOwner::Absent);
    }

    #[test]
    fn class_member_owner_sees_a_member_added_after_the_first_refresh() {
        let mut resolver = TypeResolver::new();
        let mut sub = class_model("mod.Sub", &["mod.Sub"], &[]);
        model::refresh_snapshot(&sub, &mut resolver).unwrap();
        sub.members
            .insert("shape".to_string(), Member::ClassVar(object_type()));
        let owner = class_member_owner(&mut resolver, &sub, "shape");
        let in_sub = MemberOwner::DefinedIn("mod.Sub".to_string());
        assert_eq!(owner.unwrap(), in_sub);
    }

    #[test]
    fn resolve_import_from_answers_every_name_in_source_order() {
        let resolver = resolver_with_module("pkg", &["a", "b"]);
        let owners = resolve_import_from(&resolver, "pkg", &["a", "b"]);
        let expected = vec![
            SymbolOwner::InModule("pkg".to_string()),
            SymbolOwner::InModule("pkg".to_string()),
        ];
        assert_eq!(owners.unwrap(), expected);
    }

    #[test]
    fn resolve_import_from_rejects_an_unrecorded_module() {
        let resolver = TypeResolver::new();
        let err = resolve_import_from(&resolver, "pkg", &["a"]).unwrap_err();
        assert!(err.contains("from pkg import a"), "message was: {err}");
    }

    #[test]
    fn rewrite_signature_any_rewrites_the_parameters_and_the_return() {
        let sig = sig_with(any(EXPLICIT), any(EXPLICIT));
        let rewritten = rewrite_signature_any(&sig, AnyRewrite::NonExplicit);
        assert_eq!(any_kind(&rewritten.params[0].1), SPECIAL_FORM);
        assert_eq!(any_kind(&rewritten.ret), SPECIAL_FORM);
    }

    #[test]
    fn rewrite_signature_any_leaves_a_kind_it_does_not_target() {
        let sig = sig_with(any(FROM_ERROR), any(FROM_ERROR));
        let rewritten = rewrite_signature_any(&sig, AnyRewrite::NonExplicit);
        assert_eq!(any_kind(&rewritten.params[0].1), FROM_ERROR);
        let unimported = sig_with(any(FROM_UNIMPORTED), any(FROM_UNIMPORTED));
        let rewritten = rewrite_signature_any(&unimported, AnyRewrite::NonUnimported);
        assert_eq!(any_kind(&rewritten.ret), SPECIAL_FORM);
    }

    #[test]
    fn configure_class_bases_answers_every_base_in_source_order() {
        let mut resolver = TypeResolver::new();
        let left = class_model("mod.Left", &["mod.Left"], &[]);
        model::refresh_snapshot(&left, &mut resolver).unwrap();
        let right = class_model("mod.Right", &["mod.Right"], &[]);
        model::refresh_snapshot(&right, &mut resolver).unwrap();
        let sub = subclass_of("mod.Sub", &["mod.Left", "mod.Right"]);
        let opts = BaseClassOptions::default();
        let decided = configure_class_bases(&resolver, &sub, &opts).unwrap();
        assert_eq!(decided.len(), 2);
        assert_eq!(decided[0].kind, BaseKind::Instance);
        assert_eq!(decided[1].kind, BaseKind::Instance);
    }

    #[test]
    fn configure_class_bases_reads_is_newtype_from_the_base_record() {
        let mut resolver = TypeResolver::new();
        let snapshot = TypeInfoSnapshot {
            fullname: "mod.New".to_string(),
            is_newtype: true,
            mro: vec!["mod.New".to_string()],
            ..Default::default()
        };
        resolver.insert("mod.New".to_string(), snapshot);
        let sub = subclass_of("mod.Sub", &["mod.New"]);
        let opts = BaseClassOptions::default();
        let decided = configure_class_bases(&resolver, &sub, &opts).unwrap();
        assert_eq!(decided[0].kind, BaseKind::NewTypeFail);
    }

    #[test]
    fn configure_class_bases_rejects_a_base_without_a_snapshot() {
        let resolver = TypeResolver::new();
        let sub = subclass_of("mod.Sub", &["mod.Missing"]);
        let opts = BaseClassOptions::default();
        let err = configure_class_bases(&resolver, &sub, &opts).unwrap_err();
        assert!(err.contains("mod.Missing"), "message was: {err}");
    }
}
