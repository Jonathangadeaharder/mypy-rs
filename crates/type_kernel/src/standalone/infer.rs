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

use std::collections::{HashMap, HashSet};

pub use crate::applytype::{clear_apply_reported, take_apply_reported};
pub use crate::constraints::{neg_op, Constraint, SUBTYPE_OF, SUPERTYPE_OF};
pub use crate::freshen::NATIVE_TVAR_NAMESPACE;
pub use crate::unify::UnifyOutcome;

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

// ---------------------------------------------------------------------------
// applytype.py / typevars.py: substituting a solved type argument
// ---------------------------------------------------------------------------

/// `(raw_id, meta_level, namespace)`: a type variable's identity, mirroring
/// `mypy.types.TypeVarId.__eq__` and the kernel's `expandtype::EnvKey`.
pub type TypeVarKey = (i64, i64, String);

/// `(raw_id, namespace)`: the identity `mypy.erasetype.TypeVarEraser`
/// matches an `ids_to_erase` set against, mirroring the kernel's
/// `erase_typevars::IdKey`.
pub type EraseId = (i64, String);

/// `mypy.applytype.apply_generic_arguments` (applytype.py:88-193).
///
/// `orig_types` pairs 1:1 with the callable's `variables`; a `None` entry
/// is Python's "no type argument here", which falls back to the
/// variable's default.
///
/// Python takes a `report` callback for
/// `report_incompatible_typevar_value`. The kernel replaced it with a
/// thread-local flag, so a caller that needs Python's `had_errors` verdict
/// brackets the call with [`clear_apply_reported`] and
/// [`take_apply_reported`]: `skip_unsatisfied=false` plus a violated bound
/// returns `None` and sets the flag.
pub fn apply_generic_arguments(
    callable: &Type,
    orig_types: &[Option<Type>],
    skip_unsatisfied: bool,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<Type> {
    let aliases = alias_view(resolver);
    crate::applytype::apply_generic_arguments_inner(
        callable,
        orig_types,
        skip_unsatisfied,
        strict_optional,
        resolver,
        &aliases,
    )
}

/// `get_target_type` outcome (applytype.py:33-85).
#[derive(Debug, Clone, PartialEq)]
pub enum TargetType {
    /// The argument satisfies the variable's values or upper bound.
    Determined(Type),
    /// `skip_unsatisfied` is set and the constraint is not met, so the
    /// variable keeps its default (Python's inner `None`).
    Skipped,
}

/// `mypy.applytype.get_target_type` (applytype.py:33-85): validate one
/// type argument against a type variable's values or upper bound,
/// promoting a subtype of an allowed value to that value.
///
/// `id_to_type` is Python's `dict[TypeVarId, Type]` of already-applied
/// arguments, through which an ambiguous `Never` argument gradually
/// expands the variable's default.
pub fn get_target_type(
    tvar: &Type,
    type_arg: &Type,
    skip_unsatisfied: bool,
    id_to_type: &HashMap<TypeVarKey, Type>,
    strict_optional: bool,
    resolver: &TypeResolver,
) -> Option<TargetType> {
    let aliases = alias_view(resolver);
    let decided = crate::applytype::get_target_type(
        tvar,
        type_arg,
        skip_unsatisfied,
        resolver,
        &aliases,
        id_to_type,
        strict_optional,
    )?;
    Some(match decided {
        None => TargetType::Skipped,
        Some(t) => TargetType::Determined(t),
    })
}

/// `mypy.typevars.has_no_typevars` (typevars.py:77-84): whether a type is
/// its own `erase_typevars` result.
pub fn has_no_typevars(typ: &Type) -> Option<bool> {
    crate::applytype::has_no_typevars_inner(typ)
}

// ---------------------------------------------------------------------------
// expandtype.py: substituting type variables
// ---------------------------------------------------------------------------

/// `mypy.expandtype.expand_type` (`ExpandTypeVisitor`, expandtype.py:60+).
///
/// Substitutes every type variable whose [`TypeVarKey`] is in `env`. A
/// variable absent from `env` keeps its node, exactly as Python's
/// `visit_type_var` does; an `Instance` replacement loses its
/// `last_known_value`, as Python does at expandtype.py:246-249.
pub fn expand_type(
    typ: &Type,
    env: &HashMap<TypeVarKey, Type>,
    strict_optional: bool,
) -> Option<Type> {
    crate::expandtype::expand_type_inner(typ, env, strict_optional)
}

/// [`expand_type`] under the kernel's identity contract: a result that
/// still carries a type variable, or a type-alias node a caller could not
/// re-link, defers instead of being returned.
pub fn expand_type_vars(
    typ: &Type,
    env: &HashMap<TypeVarKey, Type>,
    strict_optional: bool,
) -> Option<Type> {
    crate::expandtype::expand_type_with_env(typ, env, strict_optional)
}

/// `mypy.expandtype.expand_type_by_instance` (expandtype.py:295-325):
/// bind a member type's variables to the receiver instance's arguments,
/// keyed by the class's `type_var_raw_ids` in the instance's namespace.
pub fn expand_type_by_instance(
    typ: &Type,
    instance: &Type,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<Type> {
    crate::expandtype::expand_type_by_instance_core(typ, instance, resolver, strict_optional)
}

/// [`expand_type_by_instance`] with leftover type variables returned
/// instead of deferred, mirroring `freeze_all_type_vars`
/// (typeops.py:2102), which reifies a method's own variables afterwards.
pub fn expand_type_by_instance_free(
    typ: &Type,
    instance: &Type,
    resolver: &TypeResolver,
    strict_optional: bool,
) -> Option<Type> {
    crate::expandtype::expand_type_by_instance_free(typ, instance, resolver, strict_optional)
}

/// `mypy.expandtype.remove_trivial` (expandtype.py:984-1011): the trivial
/// union simplifications that need no `is_subtype`.
pub fn remove_trivial(types: &[Type], strict_optional: bool) -> Vec<Type> {
    crate::expandtype::remove_trivial(types, strict_optional)
}

// ---------------------------------------------------------------------------
// expandtype.py / join.py: freshening type variables
// ---------------------------------------------------------------------------

/// `freshen_all_functions_type_vars` result: the rewritten type, the
/// advanced raw-id counter and Python's `changed` flag.
#[derive(Debug, Clone, PartialEq)]
pub struct Freshened {
    pub typ: Type,
    pub next_raw_id: i64,
    pub changed: bool,
}

/// `mypy.expandtype.freshen_all_functions_type_vars`
/// (expandtype.py:416-424) with its `FreshenCallableVisitor`
/// (expandtype.py:427-435): every generic callable in the tree gets fresh
/// meta-level-1 variables.
///
/// Python mutates the global `TypeVarId.next_raw_id`. The standalone path
/// takes the counter as `start_raw_id` and returns the advanced value in
/// [`Freshened`], so the caller owns its id space instead of sharing a
/// process-global one.
pub fn freshen_all_functions_type_vars(
    typ: &Type,
    start_raw_id: i64,
    strict_optional: bool,
) -> Option<Freshened> {
    let mut next_raw_id = start_raw_id;
    let mut changed = false;
    let freshened =
        crate::freshen::freshen_type(typ, &mut next_raw_id, &mut changed, strict_optional)?;
    Some(Freshened {
        typ: freshened,
        next_raw_id,
        changed,
    })
}

/// `freshen_function_type_vars` result: the rewritten callable and the
/// advanced raw-id counter.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshenedVars {
    pub typ: Type,
    pub next_raw_id: i64,
}

/// `mypy.expandtype.freshen_function_type_vars` (expandtype.py:413-432):
/// replace one callable's declared type variables with fresh
/// meta-level-1 ones, expanding each default through the variables
/// freshened before it.
pub fn freshen_function_type_vars(callee: &Type, start_raw_id: i64) -> Option<FreshenedVars> {
    let mut next_raw_id = start_raw_id;
    let freshened = crate::freshen::freshen_function_type_vars(callee, &mut next_raw_id)?;
    Some(FreshenedVars {
        typ: freshened,
        next_raw_id,
    })
}

/// `mypy.join.match_generic_callables` (join.py:1292-1317): renumber two
/// generic callables into one shared id space before joining or meeting
/// them.
///
/// Ids come from the resolver's native registry and carry
/// [`NATIVE_TVAR_NAMESPACE`], never a caller-held counter, so the two
/// results are comparable to each other and to nothing else.
pub fn match_generic_callables(
    t: &Type,
    s: &Type,
    resolver: &TypeResolver,
) -> Option<(Type, Type)> {
    crate::freshen::renumber_generic_pair(t, s, resolver)
}

// ---------------------------------------------------------------------------
// erasetype.py: replacing type variables
// ---------------------------------------------------------------------------

/// `mypy.erasetype.erase_typevars` (`TypeVarEraser`, erasetype.py:204-285).
///
/// `ids_to_erase` is `None` to erase every type variable, or the set of
/// [`EraseId`]s to erase; `replacement` is what they become, normally
/// [`any_special_form`].
pub fn erase_typevars(
    typ: &Type,
    ids_to_erase: Option<&HashSet<EraseId>>,
    replacement: &Type,
) -> Option<Type> {
    crate::erase_typevars::erase_typevars_inner(typ, ids_to_erase, replacement)
}

/// `mypy.erasetype.replace_meta_vars` (erasetype.py:199-201): replace
/// only the meta-level type variables, the ones a meta-level inference
/// pass allocated.
pub fn replace_meta_vars(typ: &Type, target: &Type) -> Option<Type> {
    crate::erase_typevars::replace_meta_vars_inner(typ, target)
}

/// The `AnyType(TypeOfAny.special_form)` node `erase_typevars` replaces a
/// type variable with (types.py:309, value 6).
pub fn any_special_form() -> Type {
    crate::erase_typevars::make_any()
}

// ---------------------------------------------------------------------------
// subtypes.py: unification
// ---------------------------------------------------------------------------

/// `mypy.subtypes.unify_generic_callable` (subtypes.py:2954-3011).
///
/// Tri-state, mirroring the Python call site (subtypes.py:2590-2595):
/// [`UnifyOutcome::Unified`] continues with the unified left,
/// [`UnifyOutcome::NoUnify`] is Python's `unified is None` arm, and
/// [`UnifyOutcome::Defer`] means the kernel cannot decide — which on the
/// standalone path is a loud rejection, never a `False`.
///
/// Python reads the ambient `type_state.infer_unions` (typestate.py:110).
/// The standalone path takes it as an explicit parameter and installs the
/// kernel's RAII mirror for the duration of the call, the way the
/// `rust_infer_function_type_arguments` seam entry does.
pub fn unify_generic_callable(
    left: &Type,
    right: &Type,
    ignore_return: bool,
    strict_optional: bool,
    infer_unions: bool,
    resolver: &TypeResolver,
) -> UnifyOutcome {
    let _guard = crate::unify::InferUnionsGuard::install(infer_unions);
    let aliases = alias_view(resolver);
    crate::unify::unify_generic_callable_core(
        left,
        right,
        ignore_return,
        strict_optional,
        resolver,
        &aliases,
    )
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

    fn callables() -> TypeResolver {
        resolver_with(&[
            "builtins.function",
            "builtins.int",
            "builtins.object",
            "builtins.str",
        ])
    }

    /// A resolver holding `m.Box` with one class type variable whose raw
    /// id is 1, which is what `expand_type_by_instance` keys its env on.
    fn box_resolver() -> TypeResolver {
        let mut r = resolver_with(&["m.Box"]);
        let mut s = TypeInfoSnapshot {
            fullname: "m.Box".to_string(),
            name: "Box".to_string(),
            ..Default::default()
        };
        s.mro.push("m.Box".to_string());
        s.mro.push("builtins.object".to_string());
        s.has_base.insert("m.Box".to_string());
        s.has_base.insert("builtins.object".to_string());
        s.type_var_raw_ids.push(1);
        r.insert("m.Box".to_string(), s);
        r
    }

    fn tvar_ns(raw_id: i64, namespace: &str) -> Type {
        let mut t = tvar(raw_id);
        if let Type::TypeVarType { namespace: ns, .. } = &mut t {
            *ns = namespace.to_string();
        }
        t
    }

    fn meta_tvar(raw_id: i64) -> Type {
        let mut t = tvar(raw_id);
        if let Type::TypeVarType { meta_level, .. } = &mut t {
            *meta_level = 1;
        }
        t
    }

    fn unbound() -> Type {
        Type::UnboundType {
            name: "X".to_string(),
            args: Vec::new(),
            original_str_expr: None,
            original_str_fallback: None,
            optional: false,
            empty_tuple_index: false,
        }
    }

    fn param_spec() -> Type {
        Type::ParamSpecType {
            prefix: Box::new(crate::wire::Parameters {
                arg_types: Vec::new(),
                arg_kinds: Vec::new(),
                arg_names: Vec::new(),
                variables: Vec::new(),
                imprecise_arg_kinds: false,
                is_ellipsis_args: false,
            }),
            name: "P".to_string(),
            fullname: "m.P".to_string(),
            raw_id: 1,
            namespace: String::new(),
            flavor: 0,
            upper_bound: Box::new(object_ty()),
            default: Box::new(Type::AnyType {
                type_of_any: 4,
                source_any: None,
                missing_import_name: None,
            }),
            meta_level: 0,
        }
    }

    /// A callable declaring `variables` and taking `args`, the shape the
    /// apply, freshen and unify operations work on.
    fn generic_callable(variables: Vec<Type>, args: Vec<Type>, ret: Type) -> Type {
        let arg_kinds: Vec<i64> = vec![0; args.len()];
        let arg_names: Vec<Option<String>> = vec![None; args.len()];
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
            arg_types: args,
            arg_kinds,
            arg_names,
            ret_type: Box::new(ret),
            name: None,
            variables,
            type_guard: None,
            type_is: None,
            special_sig: None,
            definition_ref: None,
        }
    }

    /// The (raw_id, namespace) pairs of a callable's declared variables.
    fn tvar_keys(t: &Type) -> Vec<(i64, String)> {
        match t {
            Type::CallableType { variables, .. } => variables
                .iter()
                .filter_map(|v| match v {
                    Type::TypeVarType {
                        raw_id, namespace, ..
                    } => Some((*raw_id, namespace.clone())),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
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
