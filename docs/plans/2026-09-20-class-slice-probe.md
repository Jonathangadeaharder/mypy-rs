# Class/member slice Step 0: grown-corpus typeinfo read-closure (#114)

Status: measurement only, no production change. Verdict: **GO** on both
pre-registered clauses. The reader-fixed class/member slice consults **15
distinct TypeInfos** (56 fact-surface pairs) at family scope, against the
cold self-check's **1755** (28,065), so **R = 15 / 1755 = 0.0085 < 0.10**,
and the absolute clause holds at 15 <= 200. The bounded-subset conjecture
survives its first realistic step with roughly an order of magnitude of
headroom on both clauses. Per the reader's ordering, this is deliberately
the last small-corpus experiment: the producer (`typeinfo-from-stubs`) is
**not** started, and the next work is the skeleton implementation of the
slice, then growth by semantic slices.

Companion: issue #114; reader verdicts
`docs/reports/research-report-mypy-rs-rust-parity-followup-3-2026-09-20.md`
(question 1); pre-registered gate and methodology
`docs/plans/2026-09-20-skeleton-bootstrap-step0.md` (the trivial-corpus
predecessor); architecture `docs/adrs/0008-standalone-path-ownership.md`.

- Head: `origin/main` = `ca6a95923` at counted-run time, worktree
  `worktrees/mypy-rs-114` (branch `measure/skel-class-slice`). No Rust
  change, no `mypy/` change; one new corpus choice in the existing
  default-nothing probe driver `misc/skeleton_bootstrap_step0_probe.py`.
- Extensions: shared scratch `/private/tmp/mypy-rs-local-typekernel/`
  (built 2026-09-20 02:09, after the last commit touching
  `crates/type_kernel`), `/private/tmp/mypy-rs-local-ast/`,
  `/private/tmp/mypy-rs-local-resolver/`; no commit after their builds
  touches those crates. `scripts/assert_worktree_import.py` exit 0 before
  every counted run, from inside the worktree.
- Evidence: `/private/tmp/skel-step0/class-slice-run/` (per-corpus probe
  logs with `/usr/bin/time -l`), runner scripts
  `/private/tmp/skel-step0/run_class_slice.sh`,
  `/private/tmp/skel-step0/run_selfcheck.sh`.
- Counters are set sizes and counts, load-invariant; walls and RSS are
  recorded, never gated, and probe runs are never instruction baselines
  (the #71/#81 probe-pin discipline).

## The corpus (reader-fixed slice)

Two modules, both checked (family scope), mypy-clean, exit 0,
`Success: no issues found in 1 source file` on every counted run:

- `class_slice_base.py`: a plain class (`Shape`: class attribute, `__init__`
  instance attribute, two methods) and a pre-PEP-695 generic class
  (`Sized(Generic[T])`: generic attribute, generic return, factory method).
- `class_slice.py`: imports both base classes from the sibling; a subclass
  overriding two methods and calling `super()` in one
  (`Circle(Shape)`); a generic subclass overriding a generic method
  (`NamedBox(Sized[T])`); two module functions taking the base and the
  generic base (`combined_area`, `relabelled`); module-level member
  traffic (constructor calls, method calls, attribute reads, a class
  attribute read, a base-typed view call).

Outside the slice by construction: async functions, decorated functions,
PEP 695 syntax (the #93 hard-reject set); no loops, no conditionals, no
other statement forms, so the slice stays inside the reader's feature list.

## The pre-registered gate (from #114, in force)

> GO iff ALL of: R < 0.10 (R = distinct TypeInfos consulted by the slice
> corpus divided by 1755, the cold self-check closure); closure <= 200
> distinct TypeInfos; diagnostics byte-identical to Python mypy for every
> construct the skeleton claims to support. NO-GO otherwise.

## Measurement

| leg | closure (fullnames) | pairs | ratio to self-check |
| --- | ---: | ---: | ---: |
| class-slice (family scope) | **15** | 56 | 0.0085 |
| trivial (transfer check) | 8 | 10 | 0.0046 |
| empty-control (hygiene) | 0 | 0 | 0 |
| cold self-check (denominator) | 1755 | 28,065 | 1 |

Class-slice closure by top-level package: `abc` 1, `builtins` 5,
`class_slice` 2, `class_slice_base` 2, `typing` 5.

The 15 fullnames: `abc.ABCMeta`; `builtins.float`, `builtins.function`,
`builtins.object`, `builtins.str`, `builtins.type`; the four corpus
classes `class_slice.Circle`, `class_slice.NamedBox`,
`class_slice_base.Shape`, `class_slice_base.Sized`; `typing.Collection`,
`typing.Container`, `typing.Iterable`, `typing.Reversible`,
`typing.Sequence`.

The growth from 8 to 15 is flat, not the breadth explosion the falsifier
guarded against: one realistic module with cross-module inheritance,
generics, overrides and member lookup adds 4 corpus TypeInfos and 7 stdlib
facts (builtins 2 to 5, typing 6 to 5, plus `abc.ABCMeta`). The 56 pairs
are the consult vocabulary on those infos (`names`, `mro`, `bases`, and
`member:<name>` reads through `get`/`get_method`/`get_containing_type_info`),
which is exactly the fixture surface the skeleton must carry: the corpus
classes contribute their `names`, `mro`, `bases` and member reads, and the
stdlib facts contribute `names`, `mro` and operator/member reads.

## Verdict and what it does and does not establish

GO. Both clauses hold with ~12x (ratio) and ~13x (absolute) headroom, so
the bounded-subset hypothesis is not falsified, and the reader's NO-GO
decision matrix (bulk producer for breadth, bounded summaries for depth,
Rust semantic database for both) does not fire.

What the measurement establishes: the **fixture demand** of a realistic
class/member check is small and nameable, measured on Python mypy checking
the corpus (the probe's counted population is the checking-phase read
closure, and the skeleton premise is that whatever the real check consults
it must carry as fixture data, ADR-0008).

What it does not establish: the byte-identical differential clause is
**vacuous today**, because the skeleton cannot check classes at all
(`ClassDef`, `Import`, parameters, attribute access and calls are
hard-rejected names in `crates/mypy-rs-skel/src/subset.rs`). The skeleton's
claimed-support set for this slice is empty, so nothing can differ. The
differential becomes live when the slice implementation lands, and that is
the gate the implementation must pass: supported means byte-identical,
unsupported means explicit hard rejection, and the two categories may
never collapse (the #93 invariant, tracked as #115).

Caveats carried from the predecessor record: the closure is a lower bound
(the counted vocabulary is `names`/`mro`/`bases` plus three lookup
methods; reads outside it are unmeasured), and the denominator's pair
count drifted (28,065 here vs 28,040 in the predecessor) while the gated
fullname count is unchanged at 1755.

## Consequence: the next slice

The reader's ordering makes this the last small-corpus experiment. The
next lane is the skeleton implementation of the measured slice (parser
growth for class definitions, imports, parameters, attribute access and
calls; a symbol table for the corpus modules; corpus-class TypeInfo
construction and insertion into the resolver; scope-aware checking; the
byte-identical differential against Python mypy on this corpus; codegen
and fixture regeneration driven by the measured 15-TypeInfo closure
instead of the hardcoded Step-0 list). Then growth proceeds by semantic
slices, not by adding synthetic increments.
