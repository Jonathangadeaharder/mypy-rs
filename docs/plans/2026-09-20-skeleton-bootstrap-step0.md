# Skeleton-bootstrap Step 0: checking-phase typeinfo read-closure (#85)

Status: measurement only, no production change. Verdict: **GO** under the
pre-registered gate, on both clauses: the trivial corpus's checking-phase
typeinfo read-closure is **8 distinct TypeInfos** (10 fact-surface pairs)
against the cold self-check's **1755** (28,040), so
**R = 8 / 1755 = 0.0046 < 0.10**, and the absolute clause holds at
8 <= 200, 25x below the hand-auditable budget. An empty-control corpus
(a bare `pass` module) measures a 0-fact corpus closure and the same
stdlib-dominated whole-run union as the trivial corpus, which is the
evidence that the counted population is the corpus's consults and not the
import closure's own checking. This record closes the #85 Step-0
falsifier; GO unblocks the skeleton flip lane (brief section 9, Lane 2).
Companion: issue #85; design brief
`docs/plans/2026-09-19-standalone-skeleton-brief.md` (section 3 is the
pre-registered gate and the falsifier's rationale); architecture
`docs/adrs/0008-standalone-path-ownership.md`; parent #83, epic #72.

- Head: `origin/main` = `ca866368a` at counted-run time, worktree
  `worktrees/mypy-rs-skel-step0` (branch `perf/skeleton-step0`);
  no Rust change, no mypy/ change, one default-off probe driver in misc/.
  While this PR was in review, main advanced twice (`ae02900a2`, the #89
  env-gate fix, then `550888257`, the #78 residency lane); the branch
  rebased onto both, and smoke runs at each new head reproduce the
  trivial closure 8 fullnames / 10 pairs with the kernel engaged, so the
  counted evidence transfers to the merged head.
- Extensions: type_kernel built from this tree into the private scratch
  `/private/tmp/mypy-rs-local-typekernel-skel-step0/` (codesigned); the
  shared `/private/tmp/mypy-rs-local-{ast,resolver}` scratch dirs on
  PYTHONPATH (current for this head: no commit after their build touches
  those crates). `scripts/assert_worktree_import.py` exit 0 before every
  counted run, from inside the worktree.
- Corpora: the fixed trivial corpus (one module: four annotated primitive
  assignments, one annotated function returning a literal, one deliberate
  assignment error; mypy exit 1 with exactly one `Incompatible types in
  assignment` on every counted run); the empty control (one module,
  `pass`; exit 0, `Success: no issues found in 1 source file`); the cold
  self-check (`mypy_self_check.ini -n0 --no-incremental --dump-build-stats
  -p mypy -p mypyc`, single process, `MYPY_NUM_WORKERS=0`, 378 files,
  `Success: no issues found in 378 source files` on every counted run and
  on the probe-off baseline).
- Evidence: `/private/tmp/skel-step0/` (probe logs and /usr/bin/time
  outputs for both counted runs per corpus, the empty control, the
  kernel-off diagnostic leg, and the probe-off baselines).
- Counters are set sizes and counts, load-invariant; instructions and wall
  seconds are recorded, never gated, and probe runs are never used as
  instruction baselines (the #71/#81 probe-pin discipline).

## The pre-registered gate (from #85 / brief section 3, verbatim in force)

> **R** = |checking-phase typeinfo read-closure, trivial corpus| divided
> by |checking-phase typeinfo read-closure, cold self-check|.
> **NO-GO if R >= 0.10**, or if the trivial corpus's closure exceeds
> **200 distinct TypeInfos**. Either condition means the trivial check
> already consults a real fraction of the typeinfo universe (ratio) or
> exceeds the hand-auditable budget (absolute), so "bounded subset" is
> false and the skeleton shape dies. No flip lane starts.

The brief fixes what a closure is before any measurement: "the set of
`TypeInfo` facts a real check consults for this corpus" (brief section 3,
lines 100-104). The instrument below is built to that definition.

## What the probe counts (the instrument, stated before the numbers)

`misc/skeleton_bootstrap_step0_probe.py` runs the corpora with
runtime-only patches (nothing in mypy/ or crates/ changes; the probe
exists only while this driver runs it):

- **Consult vocabulary, as fixed by the brief**: property wrappers over
  the `TypeInfo` read surfaces `names`, `mro`, `bases`, plus wrappers over
  the kernel's member-lookup methods `get`, `get_method`,
  `get_containing_type_info`. Recorded per read: the consultee's fullname
  and the (fullname, surface) pair.
- **Arming** at the first `type_check_first_pass` call (the F3 wire-cache
  boundary, `mypy/build.py:4801-4808`), so semanal's builtins analysis,
  which every real run performs, is excluded by construction. Reads
  before arming do not exist for the probe.
- **Window attribution**: a read is counted to the module whose
  `type_check_first_pass` / `type_check_second_pass` frame is on the
  stack. Per-SCC kernel snapshot re-feeds and later SCCs' semanal (both
  post-arming readers on this head) fall outside any checker window and
  are reported only as the armed-closure diagnostic.
- **Corpus closure at family scope**: the union of checker-window closures
  over the corpus family's modules, the modules mypy was asked to check
  (the `-p` packages for the self-check; the corpus module itself for the
  file corpora). mypy also checks every stdlib stub in the import
  closure; the skeleton premise ingests those as fixture data and never
  checks them, so their own windows are not consults the skeleton's
  semanal performs. The empty control makes this measurable instead of
  argued: a zero-content corpus module produces a 0-fact corpus closure
  and the byte-same whole-run union (593 fullnames, 2,532 pairs) as the
  trivial corpus. A population that is identical for a corpus which
  consults nothing carries no corpus signal; the pre-registered wording
  ("for this corpus") is what family scope implements.
- **Refusal guards on every exit path**: run failed or unexpected output,
  arming boundary never crossed, zero checker-phase reads, empty run
  closure, corpus module never entered a checker window (window entry,
  not closure membership, so a legitimately-empty control closure cannot
  be refused), id recycling among touched infos (objects are pinned),
  native-kernel engagement mismatch. The report prints on every exit
  path; exit 0 only on the probe's own evidence. All counted runs below
  exited 0 with zero refusals.

## Probe populations (two counted runs per corpus)

| counter | trivial 1 | trivial 2 | control | selfcheck 1 | selfcheck 2 |
| --- | --- | --- | --- | --- | --- |
| corpus-family closure, fullnames | 8 | 8 | 0 | 1755 | 1755 |
| corpus-family closure, pairs | 10 | 10 | 0 | 28040 | 28040 |
| family modules entering windows | 1 | 1 | 1 | 378 | 378 |
| first / second pass calls | 53 / 0 | 53 / 0 | 53 / 0 | 848 / 112 | 848 / 112 |
| read events total / in-window | 60526 / 40653 | 60526 / 40653 | 60415 / 40592 | 6134691 / 4576482 | 6134691 / 4576482 |
| whole-run union (diagnostic) | 593 / 2532 | 593 / 2532 | 593 / 2532 | 3235 / 35842 | 3235 / 35842 |
| armed closure (diagnostic) | 594 / 2810 | 594 / 2810 | 594 / 2810 | 3244 / 44824 | 3244 / 44824 |
| infos pinned / id_recycled | 594 / 0 | 594 / 0 | 594 / 0 | 3253 / 0 | 3253 / 0 |
| refusals | 0 | 0 | 0 | 0 | 0 |

Spread: the two trivial runs are byte-identical; the two self-check runs
are identical outside five `--dump-build-stats` timing lines
(parse/semanal/type_check seconds). Every closure counter is identical
across runs.

The trivial corpus's 8 facts: `builtins.object`, `builtins.str`, and
`typing.{Awaitable, Collection, Container, Iterable, Reversible,
Sequence}`; the 10 pairs are the `names` reads for the annotated types
plus `builtins.str`'s `mro` and `names` on the deliberate-error path.
These are the facts the checker consults while processing the module's
annotations and its one error, which is the skeleton's hand-auditable
fixture vocabulary for this corpus. The self-check's 1755 decompose (by
top-level package) into 1091 `mypy.*` and 217 `mypyc.*` facts plus 447
consulted stdlib and in-repo-stub facts (ast 111, builtins 54, typing 33,
`_typeshed` 18, inspect 17, and smaller sets), all read from inside
mypy/mypyc checker windows.

**Kernel-off diagnostic leg** (Python checker only, native kernel
disabled after option parsing): the trivial closure grows to 9 fullnames /
15 pairs, adding `builtins.function` and member lookups
(`__await__`, `__getattr__`, `__getattribute__`) on `str` and
`Awaitable`. The kernel-on closure is therefore not an artifact of
native-kernel engagement: the Python checker consults the same family
plus its own function-object and member lookups. The gate is evaluated on
the kernel-on leg (the default-on production configuration); the
kernel-off numbers are reported because the skeleton targets the
kernel-on architecture.

Instructions retired, recorded never gated: trivial probe-on
21.96e9 (both runs) vs probe-off baseline 20.60e9 (+6.6%);
self-check probe-on 492.93e9 / 493.96e9 vs probe-off baseline 430.35e9
(+14.5%). Peak RSS 687 MB probe-on vs 672 MB probe-off. The overhead is
higher than the #81 probe's 1.9-2.0% because this probe records every
read event (6.13M on the self-check) rather than set sizes only; counted
runs are counter sources, never baselines.

## Gate arithmetic

| clause | measured | threshold | verdict |
| --- | --- | --- | --- |
| R = trivial closure / self-check closure | 8 / 1755 = **0.0046** | < 0.10 | pass (22x margin) |
| trivial closure, distinct TypeInfos | **8** | <= 200 | pass (25x margin) |
| pairs variant (reported) | 10 / 28040 = 0.00036 | < 0.10 | pass |

**GO.** Both clauses pass with more than an order of magnitude of margin,
and the gate arithmetic is stable across the alternate denominator
readings that involve the corpus at all.

## Alternate readings, reported rather than hidden

- **Whole-run windowed union** (union over every checked module including
  the stdlib import closure): trivial 593 vs self-check 3235, so
  R = 0.183 >= 0.10 and 593 > 200. Under this population the gate fires.
  This is the reading the empty control exists to kill: a corpus module
  whose own window consults **nothing** (closure 0/0) produces the
  byte-same union, 593 fullnames and the identical 2,532 pairs, as the
  trivial corpus. The union therefore measures the stdlib stubs' own
  checking phases, which the skeleton never performs (those facts arrive
  as fixture data), and fires identically for a check that consults zero
  facts. It is not a reading of "the facts a real check consults for this
  corpus"; counting it as the corpus closure would mean declaring a bare
  `pass` module unbounded.
- **Armed closure** (every post-arming reader, including per-SCC snapshot
  re-feeds and later SCCs' semanal): trivial 594 vs self-check 3244,
  R = 0.183. Same population defect plus the readers the pre-registration
  excludes by construction (arming at the checker boundary).
- **mypy/mypyc-facts-only denominator**: counting only the 1308
  `mypy.*`/`mypyc.*` facts inside the family closure gives
  R = 8 / 1308 = 0.0061. Reported for completeness; the primary
  denominator keeps the 447 stdlib and stub facts consulted from inside
  family windows, which a skeleton checking the mypy corpus would also
  consult.

Every reading that involves the corpus's own checking windows passes the
gate; only the two stdlib-dominated unions fire, and those fire for the
empty corpus too.

## What ships

- `misc/skeleton_bootstrap_step0_probe.py`, the default-off probe driver:
  three corpora (`trivial`, `selfcheck`, `empty-control`), the
  `--kernel-off` diagnostic leg, runtime-only patches, a report on every
  exit path, and evidence-refusal guards on the pre-registered
  conditions.
- This record.

## Caveats, stated rather than hidden

- The closure is the pre-registered vocabulary's closure, a lower bound
  on total consult traffic: reads that do not route through the wrapped
  surfaces (fullname reads during F3 wire serialization, subtyping served
  inside the Rust kernel from its snapshot) are unmeasured. Both
  numerator and denominator are measured on the same vocabulary, so R
  compares like with like; an undercount that scales with corpus size
  would move both legs in the same direction.
- The 8-fact closure is the consult vocabulary, not the fixture size: the
  skeleton's fixture must carry the full data of the TypeInfos those
  facts name (at minimum the `names`/`mro`/`bases` entries the checker
  reads). The 200-clause was pre-registered over distinct consulted
  TypeInfos, which is what was counted.
- The self-check denominator's family scope includes the stdlib facts
  consulted from inside mypy/mypyc windows (447 of 1755); excluding them
  only lowers R (0.0061), so the verdict is robust to that boundary
  choice. The numerator has no such choice: its family is the single
  corpus module.
- The empty control measures 0/0 with the kernel engaged, so the trivial
  corpus's 8 facts are caused by its content (the annotations and the
  deliberate error), not by baseline checker or kernel traffic.
- Probe overhead is 6.6% (trivial) and 14.5% (self-check) instructions,
  measured against probe-off baselines on the same head and PYTHONPATH;
  probe runs are counter sources only. Wall clocks in the evidence logs
  are load-dependent and not evidence.
- The self-check corpus module id `mypy` (the package `__init__`) has its
  own 0-fact window; the family union over all 378 mypy/mypyc windows is
  the denominator. The per-module table in each evidence log carries the
  full decomposition.

## Reproduction

```bash
# extensions: build type_kernel into the private scratch (AGENTS.md order)
cargo rustc -p mypy-type-kernel --features extension-module --lib \
  --crate-type cdylib --release -- -C link-arg=-undefined -C link-arg=dynamic_lookup
cp target/release/libtype_kernel.dylib \
  /private/tmp/mypy-rs-local-typekernel-skel-step0/type_kernel.cpython-313-darwin.so
codesign -f -s - /private/tmp/mypy-rs-local-typekernel-skel-step0/*.so

export PYTHONPATH=/private/tmp/mypy-rs-local-typekernel-skel-step0:\
/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver
# from the worktree root, before every counted run:
.venv/bin/python scripts/assert_worktree_import.py .

# counted probe runs (pool: run 1 for the file corpora, run 2 for the self-check)
/usr/bin/time -l .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus trivial
/usr/bin/time -l .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus empty-control
/usr/bin/time -l .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus trivial --kernel-off
/usr/bin/time -l .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus selfcheck

# probe-off baselines
/usr/bin/time -l .venv/bin/python -m mypy --config-file \
  /private/tmp/skel-step0/corpus/trivial_mypy.ini -n0 --no-incremental \
  --no-error-summary /private/tmp/skel-step0/corpus/trivial_step0.py
/usr/bin/time -l .venv/bin/python -m mypy --config-file mypy_self_check.ini \
  -n0 --no-incremental -p mypy -p mypyc
```
