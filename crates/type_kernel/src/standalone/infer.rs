//! Standalone-path public API: infer.
//!
//! Type inference: constraint generation, solving, substitution,
//! expansion, freshening, unification.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of constraints.rs, constraints_filter.rs, constraints_helpers.rs,
//! constraints_select.rs, solve.rs, applytype.rs, expandtype.rs,
//! freshen.rs, unify.rs, erase_typevars.rs, infer.rs rather than
//! reimplementing them; where a helper needs a Python-side callback
//! today, take the callback as a Rust trait object or an explicit record
//! and say so in the doc comment.
//!
//! Owned exclusively by the `infer` lane for this wave. Add
//! `#[cfg(test)]` unit tests here: they keep the lifted API honest and
//! are the only thing that exercises it before the driver integration
//! wave.
//!
//! # Semantic-fact store
//!
//! Every operation takes `&TypeResolver`, the store `crate::skeleton_api`
//! already exposes and the skeleton `Driver` already owns, so this area
//! invents no fact store of its own. The alias snapshots come off the
//! same resolver (`TypeResolver::aliases`, which the hybrid installs
//! through `NativeTypeResolver::new`); a resolver nobody installed them
//! on gets an empty alias view, so every alias-bearing shape defers the
//! way a missing snapshot defers in the hybrid instead of resolving
//! wrong.
//!
//! # Deferral
//!
//! `None` keeps its kernel meaning on every operation below: the ported
//! logic reached a shape it does not model. On the standalone path there
//! is no Python to fall through to, so the driver must turn `None` into
//! a loud rejection naming the construct (semantic-diff = 0). It must
//! never read `None` as "no constraints" or as "no solution".
//!
//! # Python callbacks
//!
//! `mypy.solve.solve_constraints` and `mypy.applytype` take Python
//! callbacks (`report_incompatible_typevar_value`) and read module state
//! (`type_state.infer_unions`, `type_state.infer_polymorphic`). The
//! kernel already replaced both with Rust-side channels: the report
//! callback is the `applytype` reported flag, and the module state is an
//! RAII guard. Those channels are re-exposed here rather than
//! reintroducing a callback parameter.

pub use crate::constraints::{neg_op, Constraint, SUBTYPE_OF, SUPERTYPE_OF};

use crate::aliases::TypeAliasResolver;
use crate::typeinfo::TypeResolver;
use crate::wire::Type;

/// The alias view of `resolver` for one public call.
///
/// `TypeResolver::aliases` is the shared snapshot the hybrid installs
/// (`NativeTypeResolver::new` -> `install_aliases`), so taking the view
/// is an `Arc` refcount bump, not a copy. With nothing installed the
/// empty resolver makes every `TypeAliasType` defer, which is the honest
/// answer on a path with no Python fallback to expand it.
fn alias_view(resolver: &TypeResolver) -> TypeAliasResolver {
    match resolver.aliases() {
        Some(shared) => TypeAliasResolver::from_shared_view(shared),
        None => TypeAliasResolver::new(),
    }
}

/// `mypy.constraints.infer_constraints` (the wrapper whose `erase_types`
/// default is constraints.py:802).
///
/// Lifts `constraints::infer_constraints_full_inner` with the Python
/// wrapper's own defaults, `skip_neg_op=False` and `erase_types=True`.
/// `direction` is [`SUBTYPE_OF`] or [`SUPERTYPE_OF`]; `template` is the
/// formal side and `actual` the value side, exactly as the Python call
/// sites order them.
pub fn infer_constraints(
    template: &Type,
    actual: &Type,
    direction: i64,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<Vec<Constraint>> {
    infer_constraints_full(
        template,
        actual,
        direction,
        resolver,
        strict_optional,
        false,
        true,
    )
}

/// `mypy.constraints._infer_constraints` (dispatch at constraints.py:470,
/// body at 815-943) with both wrapper flags exposed.
///
/// This is the ported `ConstraintBuilderVisitor`: proper-form expansion,
/// union normalization, the `type[...]`-union fixup, the TypeVar template
/// emit, the actual-TypeVar rebinding, the four union branches and the
/// per-shape `visit_*` arms.
#[allow(clippy::too_many_arguments)]
pub fn infer_constraints_full(
    template: &Type,
    actual: &Type,
    direction: i64,
    resolver: &TypeResolver,
    strict_optional: bool,
    skip_neg_op: bool,
    erase_types: bool,
) -> Option<Vec<Constraint>> {
    let aliases = alias_view(resolver);
    crate::constraints::infer_constraints_full_inner(
        template,
        actual,
        direction,
        resolver,
        &aliases,
        strict_optional,
        skip_neg_op,
        erase_types,
    )
}

/// `mypy.constraints._infer_constraints`'s first branch: a top-level
/// `TypeVarType` template emits exactly one constraint against the
/// actual, and every other template shape defers.
///
/// The cheap entry a driver can try before paying for the full dispatch;
/// a union or alias operand defers here because Python normalizes unions
/// before emitting, which this branch deliberately does not do.
pub fn infer_constraints_typevar_template(
    template: &Type,
    actual: &Type,
    direction: i64,
) -> Option<Constraint> {
    crate::constraints::infer_constraints_inner(template, actual, direction)
}

/// `infer_constraints_if_possible` outcome (constraints.py:980-1002).
///
/// Python signals "unsatisfiable" with an inner `None`, which would
/// collide with this module's deferral `None`; splitting the two keeps
/// the outer `Option` uniformly "deferred".
#[derive(Debug, Clone, PartialEq)]
pub enum InferIfPossible {
    /// The erased-template/actual subtype gate rejected the pair, so no
    /// constraint may be emitted (Python's inner `None`).
    Unsatisfiable,
    /// The inferred constraints (Python's list).
    Constraints(Vec<Constraint>),
}

/// `mypy.constraints.infer_constraints_if_possible`
/// (constraints.py:980-1002): the satisfiability gates that run through
/// `erase_typevars` + `is_subtype` before the recursive inference.
pub fn infer_constraints_if_possible(
    template: &Type,
    actual: &Type,
    direction: i64,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<InferIfPossible> {
    let aliases = alias_view(resolver);
    let gated = crate::constraints::infer_constraints_if_possible_inner(
        template,
        actual,
        direction,
        resolver,
        &aliases,
        strict_optional,
    )?;
    Some(match gated {
        None => InferIfPossible::Unsatisfiable,
        Some(cs) => InferIfPossible::Constraints(cs),
    })
}

/// `mypy.constraints.infer_callable_arguments_constraints`
/// (constraints.py:2032-2102): the actual-to-formal argument pairing that
/// turns two callables into per-argument constraint pairs, including the
/// star-versus-star, star-actual and kwargs-actual phases.
pub fn infer_callable_arguments_constraints(
    template: &Type,
    actual: &Type,
    direction: i64,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<Vec<Constraint>> {
    let aliases = alias_view(resolver);
    crate::constraints::infer_callable_arguments_constraints_core(
        template,
        actual,
        direction,
        resolver,
        &aliases,
        strict_optional,
    )
}

/// `mypy.constraints.infer_directed_arg_constraints`
/// (constraints.py:1909-1921): argument contravariance, i.e. `direction`
/// is inverted before the pair is inferred. A `ParamSpecType` or
/// `UnpackType` on either side yields no constraints, per Python.
pub fn infer_directed_arg_constraints(
    left: &Type,
    right: &Type,
    direction: i64,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<Vec<Constraint>> {
    let aliases = alias_view(resolver);
    crate::constraints::infer_directed_arg_constraints_native(
        left.clone(),
        right.clone(),
        direction,
        resolver,
        &aliases,
        strict_optional,
    )
}

/// `mypy.constraints.any_constraints` (constraints.py:853-908): the
/// recursive option-merge that deduces what a collection of candidate
/// constraint lists implies. `eager` is Python's flag of the same name,
/// and a `None` option is Python's "this candidate inferred nothing
/// usable".
pub fn any_constraints(
    options: Vec<Option<Vec<Constraint>>>,
    eager: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Vec<Constraint>> {
    crate::constraints::run_any_constraints(options, eager, strict_optional, resolver)
}

/// `mypy.constraints.is_type_type` (constraints.py:686-703): a
/// `type[...]`, or a union whose every item is one.
pub fn is_type_type(tp: &Type) -> bool {
    crate::constraints_filter::is_type_type_inner(tp)
}

/// `mypy.constraints.unwrap_type_type` (constraints.py:686-703): the
/// inner type of a `type[...]`, or the union of the inner items of an
/// all-`type[...]` union.
pub fn unwrap_type_type(tp: &Type) -> Option<Type> {
    crate::constraints_filter::unwrap_type_type_inner(tp)
}

/// `mypy.solve.skip_reverse_union_constraints` (solve.py:858-884): drop
/// the constraints a polymorphic solve would double-count because they
/// were inferred from a union carrying the origin variable.
pub fn skip_reverse_union_constraints(constraints: &[Constraint]) -> Option<Vec<Constraint>> {
    crate::constraints_filter::skip_reverse_union_kernel(constraints)
}

/// `mypy.solve.is_trivial_bound` (solve.py:651-655): whether a bound is
/// the wide `builtins.object` top, or `builtins.tuple` of one when
/// `allow_tuple` is set.
pub fn is_trivial_bound(t: &Type, allow_tuple: bool) -> Option<bool> {
    crate::solve::is_trivial_bound_inner(t, allow_tuple)
}

/// `mypy.solve.solve_constraints` with `allow_polymorphic=True`
/// (solve.py:241-262, 277-289).
///
/// This is the strict shape `unify_generic_callable` solves in:
/// `strict=True` and `skip_unsatisfied=False`, so a variable the solve
/// never reached comes back as ambiguous `Never` and a variable whose
/// bounds contradict comes back as `None` inside the returned list, not
/// as a deferral. `extra_tvars` on the input constraints widen the
/// solving set exactly as the polymorphic branch does.
///
/// The result pairs 1:1 with `original_vars`.
pub fn solve_constraints_polymorphic(
    original_vars: &[Type],
    constraints: &[Constraint],
    infer_unions: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Vec<Option<Type>>> {
    let aliases = alias_view(resolver);
    let solved = crate::solve::solve_constraints_poly_native(
        original_vars,
        constraints,
        infer_unions,
        strict_optional,
        resolver,
        Some(&aliases),
    );
    solved.ok()
}

/// `mypy.solve.pre_validate_solutions` (solve.py:799-829): replace a
/// solution that violates its variable's upper bound with the bound
/// itself, when that bound satisfies every constraint.
///
/// `res` must pair 1:1 with `original_vars`.
pub fn pre_validate_solutions(
    res: Vec<Option<Type>>,
    original_vars: &[Type],
    constraints: &[Constraint],
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<Vec<Option<Type>>> {
    let aliases = alias_view(resolver);
    let validated = crate::solve::pre_validate_solutions_inner(
        res,
        original_vars,
        constraints,
        &aliases,
        resolver,
        strict_optional,
    );
    validated.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::typeinfo::TypeInfoSnapshot;

    fn instance(name: &str, args: Vec<Type>) -> Type {
        Type::Instance {
            type_ref: name.to_string(),
            args,
            last_known_value: None,
            extra_attrs: None,
        }
    }

    fn int_ty() -> Type {
        instance("builtins.int", Vec::new())
    }

    fn str_ty() -> Type {
        instance("builtins.str", Vec::new())
    }

    fn object_ty() -> Type {
        instance("builtins.object", Vec::new())
    }

    fn tvar(raw_id: i64) -> Type {
        Type::TypeVarType {
            name: "T".to_string(),
            fullname: "m.T".to_string(),
            raw_id,
            namespace: String::new(),
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

    fn tvar_bound(raw_id: i64, bound: Type) -> Type {
        let mut t = tvar(raw_id);
        if let Type::TypeVarType { upper_bound, .. } = &mut t {
            *upper_bound = Box::new(bound);
        }
        t
    }

    fn type_type(item: Type) -> Type {
        Type::TypeType {
            item: Box::new(item),
            is_type_form: false,
        }
    }

    fn union(items: Vec<Type>) -> Type {
        Type::UnionType {
            items,
            uses_pep604_syntax: false,
            can_be_true: true,
            can_be_false: true,
            is_evaluated: true,
            original_str_expr: None,
            original_str_fallback: None,
        }
    }

    fn constraint(origin: Type, op: i64, target: Type) -> Constraint {
        Constraint {
            origin_type_var: origin,
            op,
            target,
            extra_tvars: Vec::new(),
        }
    }

    /// A one-positional-argument callable `(arg) -> ret`, the shape
    /// `infer_callable_arguments_constraints` pairs by position. The three
    /// argument slices must stay the same length: `formal_arguments`
    /// indexes `arg_names` by position without a length guard.
    fn callable(arg: Type, ret: Type) -> Type {
        Type::CallableType {
            fallback: Box::new(instance("builtins.function", Vec::new())),
            instance_type: None,
            is_ellipsis_args: false,
            implicit: false,
            is_bound: false,
            from_concatenate: false,
            imprecise_arg_kinds: false,
            unpack_kwargs: false,
            from_type_type: false,
            arg_types: vec![arg],
            arg_kinds: vec![0],
            arg_names: vec![None],
            ret_type: Box::new(ret),
            name: None,
            variables: Vec::new(),
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    fn resolver_with(fullnames: &[&str]) -> TypeResolver {
        let mut r = TypeResolver::new();
        for f in fullnames {
            let mut s = TypeInfoSnapshot {
                fullname: f.to_string(),
                name: f.to_string(),
                ..Default::default()
            };
            s.mro.push(f.to_string());
            s.has_base.insert(f.to_string());
            if *f != "builtins.object" {
                s.mro.push("builtins.object".to_string());
                s.has_base.insert("builtins.object".to_string());
            }
            r.insert(f.to_string(), s);
        }
        r
    }

    fn primitives() -> TypeResolver {
        resolver_with(&["builtins.int", "builtins.str", "builtins.object"])
    }

    #[test]
    fn infer_constraints_typevar_template_emits_one_constraint() {
        let c = infer_constraints_typevar_template(&tvar(1), &int_ty(), SUBTYPE_OF);
        assert_eq!(c, Some(constraint(tvar(1), SUBTYPE_OF, int_ty())));
    }

    #[test]
    fn infer_constraints_typevar_template_rejects_an_instance() {
        let c = infer_constraints_typevar_template(&int_ty(), &str_ty(), SUBTYPE_OF);
        assert_eq!(c, None);
    }

    #[test]
    fn infer_constraints_full_emits_the_typevar_template_constraint() {
        let r = primitives();
        let cs = infer_constraints(&tvar(1), &int_ty(), SUBTYPE_OF, &r, true);
        assert_eq!(cs, Some(vec![constraint(tvar(1), SUBTYPE_OF, int_ty())]));
    }

    #[test]
    fn infer_constraints_full_honours_the_direction() {
        let r = primitives();
        let cs = infer_constraints(&tvar(1), &int_ty(), SUPERTYPE_OF, &r, true);
        assert_eq!(cs, Some(vec![constraint(tvar(1), SUPERTYPE_OF, int_ty())]));
    }

    #[test]
    fn neg_op_flips_the_direction() {
        assert_eq!(neg_op(SUBTYPE_OF), SUPERTYPE_OF);
        assert_eq!(neg_op(SUPERTYPE_OF), SUBTYPE_OF);
    }

    #[test]
    fn is_type_type_accepts_a_type_and_an_all_type_union() {
        assert!(is_type_type(&type_type(int_ty())));
        assert!(is_type_type(&union(vec![
            type_type(int_ty()),
            type_type(str_ty())
        ])));
        assert!(!is_type_type(&int_ty()));
        assert!(!is_type_type(&union(vec![type_type(int_ty()), str_ty()])));
    }

    #[test]
    fn unwrap_type_type_yields_the_item_and_rejects_a_mixed_union() {
        assert_eq!(unwrap_type_type(&type_type(int_ty())), Some(int_ty()));
        let mixed = union(vec![type_type(int_ty()), str_ty()]);
        assert_eq!(unwrap_type_type(&mixed), None);
    }

    #[test]
    fn is_trivial_bound_accepts_only_object_and_opt_in_tuple() {
        assert_eq!(is_trivial_bound(&object_ty(), false), Some(true));
        assert_eq!(is_trivial_bound(&int_ty(), false), Some(false));
        let tuple = instance("builtins.tuple", vec![object_ty()]);
        assert_eq!(is_trivial_bound(&tuple, false), Some(false));
        assert_eq!(is_trivial_bound(&tuple, true), Some(true));
    }

    #[test]
    fn skip_reverse_union_constraints_drops_the_self_reverse_pair() {
        // `T <: T | int` is the shape solve.py:871-883 removes: the union
        // target carries the origin variable and the op is SUBTYPE_OF.
        let c = constraint(tvar(1), SUBTYPE_OF, union(vec![tvar(1), int_ty()]));
        let kept = skip_reverse_union_constraints(&[c]);
        assert_eq!(kept, Some(Vec::new()));
    }

    #[test]
    fn skip_reverse_union_constraints_keeps_a_plain_pair() {
        let c = constraint(tvar(1), SUBTYPE_OF, int_ty());
        assert_eq!(skip_reverse_union_constraints(&[c.clone()]), Some(vec![c]));
    }

    #[test]
    fn any_constraints_passes_a_single_valid_option_through() {
        let r = primitives();
        let c = constraint(tvar(1), SUBTYPE_OF, int_ty());
        let options = vec![Some(vec![c.clone()]), None];
        assert_eq!(any_constraints(options, false, true, &r), Some(vec![c]));
    }

    #[test]
    fn any_constraints_without_a_valid_option_is_empty() {
        let r = primitives();
        let options = vec![None, None];
        assert_eq!(any_constraints(options, false, true, &r), Some(Vec::new()));
    }

    #[test]
    fn solve_constraints_polymorphic_solves_a_single_lower_bound() {
        let r = primitives();
        let cs = vec![constraint(tvar(1), SUPERTYPE_OF, int_ty())];
        let solved = solve_constraints_polymorphic(&[tvar(1)], &cs, false, true, &r);
        assert_eq!(solved, Some(vec![Some(int_ty())]));
    }

    #[test]
    fn solve_constraints_polymorphic_leaves_a_contradicted_var_unsolved() {
        let r = primitives();
        let cs = vec![
            constraint(tvar(1), SUBTYPE_OF, int_ty()),
            constraint(tvar(1), SUPERTYPE_OF, str_ty()),
        ];
        let solved = solve_constraints_polymorphic(&[tvar(1)], &cs, false, true, &r);
        assert_eq!(solved, Some(vec![None]));
    }

    #[test]
    fn pre_validate_solutions_keeps_a_bound_satisfying_solution() {
        let r = primitives();
        let res = vec![Some(int_ty())];
        let validated = pre_validate_solutions(res, &[tvar(1)], &[], &r, true);
        assert_eq!(validated, Some(vec![Some(int_ty())]));
    }

    #[test]
    fn pre_validate_solutions_falls_back_to_the_upper_bound() {
        // T's bound is builtins.int, the proposed solution is
        // builtins.str: not a subtype, and an empty constraint list makes
        // the bound satisfy all of them, so Python pushes the raw bound.
        let r = primitives();
        let tvar_int = tvar_bound(1, int_ty());
        let validated = pre_validate_solutions(vec![Some(str_ty())], &[tvar_int], &[], &r, true);
        assert_eq!(validated, Some(vec![Some(int_ty())]));
    }

    #[test]
    fn infer_constraints_if_possible_reports_an_unsatisfiable_pair() {
        // SUPERTYPE_OF runs the second gate (constraints.py:994-1001):
        // builtins.str is not a subtype of T's bound builtins.int.
        let r = primitives();
        let tvar_int = tvar_bound(1, int_ty());
        let out = infer_constraints_if_possible(&tvar_int, &str_ty(), SUPERTYPE_OF, &r, true);
        assert_eq!(out, Some(InferIfPossible::Unsatisfiable));
    }

    #[test]
    fn infer_constraints_if_possible_infers_a_satisfiable_pair() {
        let r = primitives();
        let tvar_int = tvar_bound(1, int_ty());
        let out = infer_constraints_if_possible(&tvar_int, &int_ty(), SUPERTYPE_OF, &r, true);
        let c = constraint(tvar_int, SUPERTYPE_OF, int_ty());
        let want = InferIfPossible::Constraints(vec![c]);
        assert_eq!(out, Some(want));
    }

    #[test]
    fn infer_directed_arg_constraints_inverts_the_direction() {
        let r = primitives();
        let cs = infer_directed_arg_constraints(&tvar(1), &int_ty(), SUBTYPE_OF, &r, true);
        assert_eq!(cs, Some(vec![constraint(tvar(1), SUPERTYPE_OF, int_ty())]));
    }

    #[test]
    fn infer_callable_arguments_constraints_pairs_the_formals() {
        let r = primitives();
        let template = callable(tvar(1), int_ty());
        let actual = callable(int_ty(), int_ty());
        let cs = infer_callable_arguments_constraints(&template, &actual, SUBTYPE_OF, &r, true);
        assert_eq!(cs, Some(vec![constraint(tvar(1), SUPERTYPE_OF, int_ty())]));
    }

    #[test]
    fn infer_callable_arguments_constraints_rejects_a_non_callable() {
        let r = primitives();
        let cs = infer_callable_arguments_constraints(&int_ty(), &int_ty(), SUBTYPE_OF, &r, true);
        assert_eq!(cs, None);
    }
}
