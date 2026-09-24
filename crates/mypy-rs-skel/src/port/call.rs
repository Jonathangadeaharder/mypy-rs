//! Standalone driver glue: call.
//!
//! Adapts the skeleton's own call records to `type_kernel::standalone::call`
//! and adapts the kernel's binding answer back into the shape a diagnostic
//! needs. No checking decision is made here: which actual binds to which
//! formal, and what an unbound actual means, are the kernel's answers.
//!
//! The input record is `crate::model::Sig`, the signature the driver's micro
//! semanal resolves for a method or module function (parameters are
//! `(name, type)`, `self` is already stripped, every parameter is a required
//! positional). The call site is the typed argument list the expression pass
//! produces, so it is positional-only for the slice the driver supports; a
//! star actual cannot reach this module because `crate::subset` rejects
//! `*args` at the call site.
//!
//! Nothing here is wired into `crate::check`'s `Driver` yet: the wave-2
//! integration lane does that (docs/plans/2026-09-24-standalone-full-port-
//! wave1.md).

use type_kernel::skeleton_api::Type;
use type_kernel::standalone::call::{self, ActualArg, ArgKind, FormalParam, FormalToActual};

use crate::model::Sig;

/// The kernel `CallableType` for a skeleton signature, so the call kernel can
/// read it: every parameter becomes a required positional, the fallback is
/// `builtins.function` (a plain function, never a type object), and `name` is
/// the fullname mypy puts in `callable_name` for diagnostics.
pub fn callee_of(sig: &Sig, name: Option<&str>) -> Type {
    let mut arg_types = Vec::with_capacity(sig.params.len());
    let mut arg_kinds = Vec::with_capacity(sig.params.len());
    let mut arg_names = Vec::with_capacity(sig.params.len());
    for (param, typ) in &sig.params {
        arg_types.push(typ.clone());
        arg_kinds.push(ArgKind::Pos.as_i64());
        arg_names.push(Some(param.clone()));
    }
    Type::CallableType {
        fallback: Box::new(function_fallback()),
        instance_type: None,
        is_ellipsis_args: false,
        implicit: false,
        is_bound: false,
        from_concatenate: false,
        imprecise_arg_kinds: false,
        unpack_kwargs: false,
        from_type_type: false,
        arg_types,
        arg_kinds,
        arg_names,
        ret_type: Box::new(sig.ret.clone()),
        name: name.map(str::to_string),
        variables: Vec::new(),
        type_guard: None,
        type_is: None,
        special_sig: None,
        definition_ref: None,
    }
}

/// `builtins.function` as a bare `Instance`: the fallback mypy gives a plain
/// function's `CallableType`.
fn function_fallback() -> Type {
    Type::Instance {
        type_ref: "builtins.function".to_string(),
        args: Vec::new(),
        last_known_value: None,
        extra_attrs: None,
    }
}

/// A signature's parameters in the record shape the kernel's argument mapper
/// takes (`callee.arg_kinds` / `callee.arg_names`).
pub fn formal_params(sig: &Sig) -> Vec<FormalParam> {
    sig.params
        .iter()
        .map(|(name, _)| FormalParam::positional(name))
        .collect()
}

/// One positional actual per typed argument (`arg_kinds` / `arg_names` at the
/// call site).
pub fn actual_args(arg_types: &[Type]) -> Vec<ActualArg> {
    vec![ActualArg::positional(); arg_types.len()]
}

/// The kernel's binding of one call site, plus the actuals no parameter took.
#[derive(Debug)]
pub struct Binding {
    /// `formal_to_actual[i]`: the indices of the actuals bound to parameter
    /// `i`, exactly the mapping the rest of the call kernel consumes.
    pub formal_to_actual: FormalToActual,
    /// The actual indices no parameter bound. mypy turns these into
    /// `too_many_arguments` / `unexpected_keyword_argument`
    /// (checkexpr.py:3439-3498); the driver renders them, this record only
    /// reports which ones they are.
    pub unbound_actuals: Vec<usize>,
}

/// Bind a call site against a signature: `map_actuals_to_formals`
/// (`mypy/argmap.py`) reached through the skeleton's records.
///
/// `None` is the kernel deferring. For the positional-only shape
/// [`actual_args`] produces it cannot happen, and it stays in the signature
/// so a future star-argument call site defers instead of binding wrong.
pub fn bind(sig: &Sig, arg_types: &[Type]) -> Option<Binding> {
    let formal = formal_params(sig);
    let actual = actual_args(arg_types);
    let mapping = call::map_actuals_to_formals(&actual, &formal)?;
    let mut bound = vec![false; actual.len()];
    for actuals in mapping.as_slices() {
        for &index in actuals {
            if let Some(slot) = bound.get_mut(index as usize) {
                *slot = true;
            }
        }
    }
    let unbound_actuals = bound
        .iter()
        .enumerate()
        .filter(|(_, is_bound)| !*is_bound)
        .map(|(index, _)| index)
        .collect();
    Some(Binding {
        formal_to_actual: mapping,
        unbound_actuals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int() -> Type {
        Type::Instance {
            type_ref: "builtins.int".to_string(),
            args: Vec::new(),
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn sig(params: &[&str]) -> Sig {
        Sig {
            params: params.iter().map(|p| ((*p).to_string(), int())).collect(),
            ret: int(),
        }
    }

    fn expect_binding(binding: Option<Binding>) -> Binding {
        binding.expect("the mapping decides")
    }

    #[test]
    fn callee_of_round_trips_through_the_kernel_signature() {
        let callee = callee_of(&sig(&["a", "b"]), Some("mod.f"));
        let Some(read_back) = call::callable_signature(&callee) else {
            panic!("callee_of must build a CallableType");
        };
        assert_eq!(read_back.arg_types.len(), 2);
        assert_eq!(read_back.arg_kind(0), Some(ArgKind::Pos));
        assert_eq!(read_back.arg_names[0].as_deref(), Some("a"));
        assert_eq!(read_back.arg_names[1].as_deref(), Some("b"));
        assert_eq!(*read_back.ret_type, int());
        let dispatch = call::classify_call(&callee);
        assert_eq!(dispatch, call::CallDispatch::PlainCallable);
    }

    #[test]
    fn callee_of_handles_a_nullary_signature() {
        let callee = callee_of(&sig(&[]), None);
        let Some(read_back) = call::callable_signature(&callee) else {
            panic!("a nullary signature is still a CallableType");
        };
        assert!(read_back.arg_types.is_empty());
        assert_eq!(read_back.arg_kind(0), None);
    }

    #[test]
    fn formal_params_names_every_parameter_positional() {
        let params = formal_params(&sig(&["a", "b"]));
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], FormalParam::positional("a"));
        assert_eq!(params[1], FormalParam::positional("b"));
    }

    #[test]
    fn formal_params_of_a_nullary_signature_is_empty() {
        assert!(formal_params(&sig(&[])).is_empty());
    }

    #[test]
    fn actual_args_are_one_positional_per_typed_argument() {
        let actual = actual_args(&[int(), int()]);
        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0], ActualArg::positional());
        assert_eq!(actual[1].kind, ArgKind::Pos);
        assert!(actual_args(&[]).is_empty());
    }

    #[test]
    fn bind_maps_a_matching_positional_call() {
        let binding = expect_binding(bind(&sig(&["a", "b"]), &[int(), int()]));
        let mapped = binding.formal_to_actual.as_slices().to_vec();
        assert_eq!(mapped, vec![vec![0i64], vec![1i64]]);
        assert!(binding.unbound_actuals.is_empty());
    }

    #[test]
    fn bind_reports_the_actual_no_formal_took() {
        let binding = expect_binding(bind(&sig(&["a"]), &[int(), int()]));
        let mapped = binding.formal_to_actual.as_slices().to_vec();
        assert_eq!(mapped, vec![vec![0i64]]);
        assert_eq!(binding.unbound_actuals, vec![1usize]);
    }

    #[test]
    fn bind_reports_every_actual_unbound_for_a_nullary_callee() {
        let binding = expect_binding(bind(&sig(&[]), &[int()]));
        assert!(binding.formal_to_actual.is_empty());
        assert_eq!(binding.unbound_actuals, vec![0usize]);
    }
}
