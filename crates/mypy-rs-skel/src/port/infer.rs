//! Standalone driver glue: infer.
//!
//! Drive the kernel inference api: local variable inference, annotation
//! checking.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::infer`. It owns no checking logic of its
//! own: it adapts skeleton records (`crate::model`, `crate::fixtures`) to
//! the kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `infer` lane for this wave. Add
//! `#[cfg(test)]` unit tests here.
//!
//! # What this increment covers
//!
//! `mypy.infer.infer_type_arguments` is constraints plus a solve. The
//! constraint half is lifted here exactly, mirroring the kernel's own
//! `solve.py::infer_function_type_arguments`: one
//! `infer_constraints(formal, actual, SUPERTYPE_OF)` per formal/actual
//! pair, in mapping order, with a `None` actual contributing nothing
//! (that is mypy's two-pass shape, where the second pass re-runs with
//! the arguments the first pass could not type yet).
//!
//! The solve half a call runs is the NON-polymorphic
//! `solve_constraints` (solve.py:265-275). The kernel still answers that
//! only through its wire-blob shape (`solve::solve_constraints_native`
//! encodes the solutions and its own Rust caller decodes them straight
//! back), so it is not yet reachable from a Python-free signature. Until
//! it is, this module stops at the constraints rather than substituting
//! the polymorphic solve, which is a different algorithm
//! (solve.py:241-262) and would be a wrong answer, not a deferral.
//!
//! `expand_actual_arg` (argmap.py:269-364) is the identity for a plain
//! positional actual, which is the only argument kind the skeleton
//! subset models; a star argument is therefore rejected here instead of
//! silently losing its unpacking.

use type_kernel::skeleton_api::{Type, TypeResolver};
use type_kernel::standalone::infer::{infer_constraints, Constraint, SUPERTYPE_OF};

use crate::model::Sig;

/// Why an inference the skeleton asked for produced no answer.
///
/// Both arms are rejections the driver renders loudly. Neither is a
/// default: an inference that cannot be made exactly must never fall
/// back to a guessed type argument (semantic-diff = 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferReject {
    /// The call shape left the subset this adapter models: an arity that
    /// needs a real `formal_to_actual` mapping, or a star argument whose
    /// `expand_actual_arg` unpacking is not lifted.
    Unsupported(String),
    /// The kernel declined a shape it does not model. On the standalone
    /// path there is no Python to fall through to, so this is an internal
    /// error naming the construct, exactly like `check.rs`'s
    /// `require_decidable`.
    Deferred(String),
}

/// The constraint set for calling `formal_types` against `actual_types`,
/// in the `mypy.checkexpr.infer_function_type_arguments` shape.
///
/// A `None` actual is an argument the pass could not type yet and
/// contributes no constraint, mirroring the kernel's `None`-actual skip;
/// the caller re-runs with it filled in. `formal_to_actual` is the
/// identity, so a length mismatch is a rejection rather than a truncating
/// zip.
pub fn infer_call_constraints(
    formal_types: &[Type],
    actual_types: &[Option<Type>],
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Result<Vec<Constraint>, InferReject> {
    if formal_types.len() != actual_types.len() {
        return Err(InferReject::Unsupported(format!(
            "{} formals against {} actuals needs a formal_to_actual mapping \
             this adapter does not model",
            formal_types.len(),
            actual_types.len()
        )));
    }
    let mut constraints = Vec::new();
    for (index, (formal, actual)) in formal_types.iter().zip(actual_types).enumerate() {
        let Some(actual) = actual else {
            continue;
        };
        let inferred = infer_constraints(formal, actual, SUPERTYPE_OF, resolver, strict_optional);
        match inferred {
            Some(cs) => constraints.extend(cs),
            None => {
                return Err(InferReject::Deferred(format!(
                    "the kernel deferred the constraints for actual argument {index}"
                )));
            }
        }
    }
    Ok(constraints)
}

/// [`infer_call_constraints`] for a call whose every argument is already
/// typed, the shape `check.rs`'s generic-constructor inference reaches.
pub fn infer_sig_constraints(
    sig: &Sig,
    arg_types: &[Type],
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Result<Vec<Constraint>, InferReject> {
    let formals: Vec<Type> = sig.params.iter().map(|(_, t)| t.clone()).collect();
    let actuals: Vec<Option<Type>> = arg_types.iter().map(|t| Some(t.clone())).collect();
    infer_call_constraints(&formals, &actuals, resolver, strict_optional)
}

#[cfg(test)]
mod tests {
    use super::*;

    use type_kernel::skeleton_api::TypeInfoSnapshot;

    fn instance(name: &str) -> Type {
        Type::Instance {
            type_ref: name.to_string(),
            args: Vec::new(),
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn int_ty() -> Type {
        instance("builtins.int")
    }

    fn str_ty() -> Type {
        instance("builtins.str")
    }

    fn object_ty() -> Type {
        instance("builtins.object")
    }

    fn tvar(raw_id: i64, namespace: &str) -> Type {
        Type::TypeVarType {
            name: "T".to_string(),
            fullname: "m.T".to_string(),
            raw_id,
            namespace: namespace.to_string(),
            values: Vec::new(),
            upper_bound: Box::new(object_ty()),
            default: Box::new(Type::AnyType {
                type_of_any: 4,
                source_any: None,
                missing_import_name: None,
            }),
            variance: 0,
            meta_level: 0,
        }
    }

    fn resolver() -> TypeResolver {
        let mut r = TypeResolver::new();
        for f in ["builtins.int", "builtins.str", "builtins.object"] {
            let mut s = TypeInfoSnapshot {
                fullname: f.to_string(),
                name: f.to_string(),
                ..Default::default()
            };
            s.mro.push(f.to_string());
            s.has_base.insert(f.to_string());
            if f != "builtins.object" {
                s.mro.push("builtins.object".to_string());
                s.has_base.insert("builtins.object".to_string());
            }
            r.insert(f.to_string(), s);
        }
        r
    }

    fn sig(params: Vec<(&str, Type)>, ret: Type) -> Sig {
        Sig {
            params: params
                .into_iter()
                .map(|(name, t)| (name.to_string(), t))
                .collect(),
            ret,
        }
    }

    fn only(constraints: &[Constraint]) -> &Constraint {
        assert_eq!(constraints.len(), 1, "one formal, one constraint");
        &constraints[0]
    }

    #[test]
    fn infers_a_type_argument_from_a_positional_actual() {
        // `__init__(self, value: T)` called with an `int`: the formal is
        // the class-frame type variable, so the one constraint binds T
        // from below, which is what a solve would turn into `int`.
        let r = resolver();
        let formal = tvar(1, "m.Box");
        let formals = std::slice::from_ref(&formal);
        let actuals = [Some(int_ty())];
        let out = infer_call_constraints(formals, &actuals, &r, true);
        let cs = out.expect("a type-variable formal is decidable");
        let c = only(&cs);
        assert_eq!(c.origin_type_var, formal);
        assert_eq!(c.op, SUPERTYPE_OF);
        assert_eq!(c.target, int_ty());
    }

    #[test]
    fn a_none_actual_contributes_no_constraint() {
        let r = resolver();
        let formals = vec![tvar(1, "m.Box"), tvar(2, "m.Box")];
        let actuals = vec![Some(int_ty()), None];
        let cs = infer_call_constraints(&formals, &actuals, &r, true).expect("decidable");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].origin_type_var, formals[0]);
    }

    #[test]
    fn rejects_an_arity_that_needs_a_real_mapping() {
        let r = resolver();
        let formals = vec![tvar(1, "m.Box")];
        let actuals = vec![Some(int_ty()), Some(str_ty())];
        let out = infer_call_constraints(&formals, &actuals, &r, true);
        assert!(matches!(out, Err(InferReject::Unsupported(_))));
    }

    #[test]
    fn infers_over_a_skeleton_signature() {
        let r = resolver();
        let init = sig(vec![("value", tvar(1, "m.Box"))], object_ty());
        let cs = infer_sig_constraints(&init, &[int_ty()], &r, true).expect("decidable");
        let c = only(&cs);
        assert_eq!(c.op, SUPERTYPE_OF);
        assert_eq!(c.target, int_ty());
    }

    #[test]
    fn a_sig_arity_mismatch_is_a_rejection_not_a_truncation() {
        let r = resolver();
        let init = sig(vec![("value", tvar(1, "m.Box"))], object_ty());
        let out = infer_sig_constraints(&init, &[int_ty(), str_ty()], &r, true);
        assert!(matches!(out, Err(InferReject::Unsupported(_))));
    }

    #[test]
    fn an_untouched_pair_yields_no_constraints() {
        // Two unrelated builtins instances carry no type variable, so the
        // constraint list is empty rather than a deferral: this is the
        // shape `check.rs` reaches for a non-generic constructor.
        let r = resolver();
        let cs = infer_call_constraints(&[int_ty()], &[Some(str_ty())], &r, true);
        assert_eq!(cs, Ok(Vec::new()));
    }
}
