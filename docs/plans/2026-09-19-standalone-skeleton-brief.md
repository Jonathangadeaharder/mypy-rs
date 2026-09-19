# Standalone-skeleton brief: first Python-free check path (#83 chosen slice)

Status: design brief, second deliverable of #83. No production change in
this lane. Companion ADR: `docs/adrs/0008-standalone-path-ownership.md`
(same PR; the ADR fixes the ownership decision, this brief fixes the
slice, its pre-registered Step-0 falsifier, and the gates). Evidence base:
#71 Step 0 (PR #78, `docs/plans/2026-09-19-subtype-residency-step0.md`),
#81 Step 0 (PR #82, `docs/plans/2026-09-19-type-arena-amortization-step0.md`),
#54 / ADR-0006, #70 (PR #75), the gap attribution
(`docs/plans/2026-09-19-gap-attribution.md`), epic #72, and the code facts
cited inline.

What this brief fixes before any implementation lane: the slice and its
gate (section 1), the mechanics (section 2), the pre-registered Step-0
falsifier with hard thresholds (section 3), the deletion-framed progress
metric (section 4), the correctness gate (section 5), what the skeleton
does not claim (section 6), the risk register (section 7), non-goals
(section 8), and the lane slicing (section 9).

## 1. The slice: one trivial corpus, checked end to end with no Python

A `mypy-rs` executable that type-checks a fixed, committed corpus of one
trivial module, performing every stage in Rust: parse, bounded semantic
analysis, type checks via the kernel's Rust internals, diagnostics
rendering, exit-code semantics. At check time the process starts no
Python interpreter and imports no `mypy` Python module. That single
property is the slice: it is the first check path in the project where
only the Rust representation exists.

Corpus (committed with the flip lane, exact contents fixed there):

- one module with module-level annotated assignments over the primitive
  literal set (`int`, `str`, `bool`, `float`), one simple annotated
  function returning a literal, and one deliberate type error (an
  `int`-annotated name assigned a `str` literal) so the differential is
  not vacuous;
- supported statement subset: module-level `AssignmentStmt` with literal
  right-hand sides, `FuncDef` with fully annotated signature and a single
  literal `ReturnStmt`, `PassStmt`. Nothing else. A statement outside the
  subset is a hard error naming the limitation (fail-early), never a
  silent skip.

Why this is candidate 2 of #83 and not candidates 1 or 3: ADR-0008
Decision 1. In one line each: a hybrid-resident AST flip cannot delete the
Python node graph while Python semanal consumes it (`mypy/build.py:4613`
materializes every checked module), so it is the #71 second-copy shape; a
hybrid-resident semanal flip has its products consumed by the Python
checker, so it is either the #81 per-read-crossing shape or the #54/#71
dual-representation shape; the skeleton is the only bounded scope where
the deciding constraint ("delete a representation, Rust canonical, no
copy") is satisfiable at all, and candidates 1 and 3 live inside it
(ADR-0008 Decision 2).

**Gate name: the cargo feature `skel` on a new crate** (working name
`crates/mypy-rs-skel`, bin `mypy-rs`; final names fixed at
implementation). The bin target declares `required-features = ["skel"]`,
so no default `cargo build` ever produces it; `misc/build_wheel.py` is
untouched, so no wheel ships it. There is no `Options` entry because the
skeleton is not in the check path; production mypy cannot reach it. This
is the default-off gate. The feature is not an `Options` gate and is
excluded from `OPTIONS_AFFECTING_CACHE` by irrelevance, asserted by the
unchanged constant.

## 2. The flip, mechanically

- **Parse.** The skeleton reuses the ruff-based parse machinery behind
  `crates/ast_serialize` (the same parser `parse` drives at
  `crates/ast_serialize/src/lib.rs:535`). Sharing may need internal
  functions to become `pub` within the workspace: a visibility-only,
  behavior-free change to that crate, gated by the same `skel` feature or
  by nothing at all since visibility alone changes no behavior. The
  skeleton consumes the parsed tree; it does not use the AST wire bytes
  and does not call the PyO3 entry.
- **AST.** A small Rust AST for the supported statement subset, built from
  the ruff tree. Not `mypy.nodes`: the Python node classes do not exist
  on this path. The subset is small enough to be its own record types.
- **Semanal micro-pass.** Module symbol table for the subset: bind
  module-level names, resolve annotation targets to primitive fullnames,
  bind function signatures. The only consumer is the Rust driver.
- **Typeinfo bootstrap.** `TypeInfoSnapshot` records for exactly the
  primitive set, built in Rust from committed, generated fixtures (data
  files in the crate). The generator runs Python mypy once at fixture
  creation and is the only Python anywhere in the slice, at codegen time
  only. The fixtures are versioned data, like typeshed patches; their
  production is not part of the check path.
- **Checks.** The kernel crate consumed as rlib
  (`crates/type_kernel/Cargo.toml`, `crate-type = ["cdylib", "rlib"]`):
  literal-type construction, assignment compatibility, signature checks
  for the one function, all through ordinary Rust calls. No PyO3 seam
  executes; no wire bytes are marshaled.
- **Diagnostics.** Rendered in Rust to mypy's exact format for the one
  supported error class (assignment incompatibility), plus the success
  line. Exit code semantics match `mypy` (0 on success, 1 on errors).
- **Gate-off is exactly today.** The crate does not exist in any default
  build; production mypy, its suites, and its caches are untouched.

## 3. Pre-registered Step-0 falsifier (hard thresholds, fixed now)

The one question that can kill this slice shape before any skeleton code:
**is the bootstrap actually bounded?** The skeleton claims its semanal
subset is small because its corpus is trivial. That is an assumption about
the checking-phase typeinfo read-closure: the set of `TypeInfo` facts a
real check consults for this corpus. If a one-module corpus already
consults a large fraction of what a real build consults, no bounded
skeleton exists; the honest next lane is a Rust typeinfo-from-stubs
producer with its own design, not a skeleton that smuggles in a real
semanal.

**Falsifier, fixed before measurement:**

- **R** = |checking-phase typeinfo read-closure, trivial corpus| divided
  by |checking-phase typeinfo read-closure, cold self-check|.
- **NO-GO if R >= 0.10**, or if the trivial corpus's closure exceeds
  **200 distinct TypeInfos**. Either condition means the trivial check
  already consults a real fraction of the typeinfo universe (ratio) or
  exceeds the hand-auditable budget (absolute), so "bounded subset" is
  false and the skeleton shape dies. No flip lane starts.
- Both thresholds are fixed now, before any measurement, in the shape of
  #81 (`constructions / appearances` vs 1.0) and #71 (conversion ratio
  vs 0.5): a number that can only be argued with after the fact is not a
  falsifier.

**Priced context, reported not gated** (the #81 precedent for reporting a
delta alongside the gate): the production hybrid's remaining Python mass
that the standalone direction eventually deletes, from the phase table in
`docs/remaining-migration-plan.md`: parse 1.0% absorbed (~5.0 s absolute,
dominated by the materialization walk this path never runs), semanal
59.1% absorbed, type_check 76.6% absorbed. The skeleton itself deletes
none of it; its corpus is outside the production check, and per #72
net-positive-now is explicitly not required. The number that matters
later is `I(Rust standalone)` vs `I(upstream mypy)`, which only a
Python-free path can measure at all.

**Probe specification** (new instrument; the Step-0 lane is filed as #85,
it is not run in this lane):

- `misc/skeleton_bootstrap_step0_probe.py`, default-off, env-gated,
  runtime-only patches over `mypy.nodes.TypeInfo` read surfaces
  (class-level property wrappers on the read-mostly attributes the
  checker consults: `names`, `mro`, `bases`, `members`-style lookups per
  the kernel's consult vocabulary), recording distinct fullnames and
  (fullname, attribute) pairs. Nothing in `mypy/` or `crates/` changes.
- Arming is phase-gated at checker start, the same boundary the F3 wire
  cache uses (`type_check_first_pass`, `mypy/build.py:4801-4808`), so
  semanal's full analysis of `builtins` (which every real mypy run
  performs, even for a one-line module) is excluded by construction:
  the closure measured is what the checking phase consults, which is what
  the skeleton must be able to serve.
- Two corpora per counted run pair: the fixed trivial corpus and the cold
  self-check (`mypy_self_check.ini -n0 --no-incremental -p mypy -p mypyc`,
  single process, `MYPY_NUM_WORKERS=0`). Two counted runs per corpus;
  counters are load-invariant; probe runs are never instruction baselines
  (the #71/#81 probe-pin discipline); touched objects are pinned so id
  recycling cannot alias a closure entry.
- Evidence-refusal guards in the driver, on every exit path: run failed,
  zero checker-phase reads, closure empty, arming boundary never crossed.
  The driver prints the report on every exit path and exits 0 only on
  its own evidence.
- Pre-flight: `scripts/assert_worktree_import.py` exit 0 from inside the
  lane worktree before every counted run (#1789 lesson).

## 4. Deletion-framed progress metric (milestone semantics per #72)

With the skeleton present (gate on, which for this slice means: the
binary built with `--features skel` and run on its corpus), exactly the
following are absent from that check path, each asserted structurally in
the flip lane's test, not inferred:

1. **The Python AST node graph never materializes.** No `mypy.nodes`
   constructor runs; the process imports no `mypy` module. Today every
   checked module fully materializes (`mypy/build.py:4613`); on this path
   that representation does not exist.
2. **The AST wire round trip never runs.** Neither `serialize_suite`
   (`crates/ast_serialize/src/lib.rs:778`) encode-for-Python nor
   `mypy/nativeparse.py` byte decode executes for the corpus.
3. **Zero of the 961 registered PyO3 seams execute.** The boundary is
   absent, not avoided: the binary links no libpython (asserted with
   `otool -L` in the flip lane's test) and starts no interpreter.
4. **The Python TypeInfo graph is absent for the corpus.** The facts come
   from committed fixtures consumed in Rust; there is no Python-owned
   graph to shadow, which is the structural difference from G4, F1 and
   ADR-0006 (no mirror exists because there is nothing to mirror).

Reported alongside, not gated: total instructions and peak RSS of the
skeleton run (the `/usr/bin/time -l` discipline), the fixture sizes, and
the trivial-corpus runtime of Python mypy on the same machine for scale.
The seam count in the production check stays 961: the skeleton adds no
seam anywhere (non-goal 8.2).

## 5. Correctness gate

1. **Differential parity against Python mypy on the fixed corpus.** Real
   mypy (this tree, this venv, `--no-incremental`, cache to a scratch
   dir) versus the skeleton, same corpus: byte-identical stdout
   (including the error line for the deliberate error, its ` [assignment]`
   code, and the `Success: no issues found in 1 source file` line) and
   identical exit codes. This is the both-gate-states differential in its
   natural form: gate-off is "the binary does not exist", and production
   mypy is the oracle in both arms.
2. **Production mypy unchanged.** The flip lane touches no `mypy/` or
   `crates/` behavior (a visibility-only change inside
   `crates/ast_serialize` is permitted and must be proven behavior-free by
   the crate's own `cargo test` plus one targeted corpus file, T2). The
   native-build and parity rules apply unchanged: rebuild the extensions
   after any `crates/ast_serialize` touch (AGENTS.md build order).
3. **Cache isolation.** The skeleton writes no cache: the flip lane's
   test asserts no `.mypy_cache` appears and no file under the corpus dir
   changes. `OPTIONS_AFFECTING_CACHE` and `CACHE_VERSION` untouched
   (nothing this lane does can reach them; asserted by diff against
   main).
4. **Daemon, incremental, fine-grained, plugins.** Not claimed for the
   skeleton path (section 6); production semantics are unchanged because
   production code is unchanged. The flip lane still runs the touched
   surface's suites (`test_nativeparse.py`,
   `testtypes_native_engagement_types.py`) and one targeted corpus file
   per the T2 tier, to prove the visibility change and the new crate are
   inert for the hybrid.
5. **Self-check leg.** One cold self-check on the flip lane's head before
   the differential, proving the tree under test is the tree being
   measured (`scripts/assert_worktree_import.py` first, per standard
   discipline).

## 6. What the skeleton does not claim

- No daemon, no incremental mode, no fine-grained updates, no cache, no
  plugin execution, no parallel workers: the path has none of these
  because it has no Python host. Each is epic work that lands as the
  standalone path grows (steps 4-6).
- No semantic breadth: one module, three statement forms, one error
  class. A corpus file outside the subset fails loudly.
- No performance claim of any kind against the hybrid or against upstream
  mypy beyond the reported, not gated, numbers of section 3.
- No deletion of any production work: the milestone is the first
  Python-free path (section 4), per #72's rule that intermediate states
  are gated on deletion milestones, and this slice's deletion is
  structural absence on its own path.

## 7. Risk register

1. **Bootstrap closure is large (the Step-0 risk).** If the trivial
   corpus consults `int`'s full member table, `object` MRO walk-ins, or
   promotion machinery that fans out, the fixture producer is the real
   semanal. Mitigation: the pre-registered falsifier (section 3) kills
   the shape first; the fallback lane is a typeinfo-from-stubs producer
   with its own design.
2. **Diagnostics drift.** Byte-identical output depends on mypy's exact
   message text, which upstream changes. Mitigation: the corpus and its
   expected output are committed together; the differential fails loudly
   on drift, and the fix is regenerating the expectation, never a local
   reformat.
3. **Kernel-internal API instability.** The rlib surface is not a
   published contract; the skeleton depends on crate internals the
   hybrid only reaches through seams. Mitigation: the skeleton lives in
   the same workspace at the same version pin; a kernel refactor that
   breaks it breaks the build, visibly, which is the intended coupling.
4. **Fixture staleness.** Generated typeinfo facts can drift from
   typeshed. Mitigation: the generator is committed, runs in CI as a
   freshness check (regenerate, diff, fail on change), and the fixture
   format is data, not code.
5. **Scope creep into real semanal.** The likeliest failure of the flip
   lane is generosity: adding "just one more" statement form until the
   subset is a parser for semanal. Mitigation: the statement subset is
   fixed in this brief; extension is a new lane with its own Step-0
   question, not a rider.
6. **A second checker grows.** The skeleton's driver must not become a
   parallel implementation of checking semantics that then disagrees with
   the kernel. Mitigation: all semantic decisions route through kernel
   internals; the driver only orchestrates and renders. Where the kernel
   lacks a Rust-internal entry, the lane adds it beside the existing seam
   implementation (same crate, same logic), never a divergent copy.
7. **Windows/Linux linkage.** The no-libpython assertion is darwin-first
   (`otool -L`); other platforms get their own assert in their own lane
   when CI there exists. Recorded, not solved here.
8. **Visibility change to `crates/ast_serialize`.** The one production
   tree this slice can touch. Mitigation: visibility only, no behavior,
   gated by the crate's tests and the T2 corpus leg (section 5.2).

## 8. Explicit non-goals

1. No mirror, shadow, capture, or dual-write of any Python-owned graph.
   The path has no Python graph; the fixtures are data consumed in Rust.
2. No seam-count increase, anywhere: no new PyO3 seams, no new `Options`
   gates, no new kernel registry entries. The 961 stay 961.
3. No `OPTIONS_AFFECTING_CACHE` change, no `CACHE_VERSION` change.
4. No default flip of anything: the feature is off in every default
   build and absent from every wheel.
5. No plugin compatibility work (epic step 7), no ADR-0007 facade
   implementation; the facade design stays a recorded architecture.
6. No hybrid-resident AST or semanal flip (ADR-0008 Decision 1 forbids
   them; they are not "deferred", they are closed in their cheap forms
   by #81 and #71).
7. No new measurement infrastructure beyond the Step-0 probe, which is
   default-off, env-gated, and runtime-only.

## 9. Lane slicing

- **Lane 0 (this PR):** ADR-0008, this brief, the ADR-0007 status
  amendment. Done here.
- **Lane 1, Step 0 (measurement only):** the bootstrap read-closure
  probe and the two corpora, the R arithmetic, the verdict record. Filed
  as #85 (self-assigned); it can end the slice shape on its own numbers,
  exactly as #81 did.
- **Lane 2 (the flip, feature-gated):** the crate, the subset AST, the
  semanal micro-pass, the fixtures plus generator, the driver, the
  renderer, the differential test, the no-libpython assert. Estimated:
  2-3 build-class lanes; T2 blast radius (new crate plus one
  visibility-only touch).
- **Lane 3 (the verdict):** the Step-3 reported numbers on the merged
  skeleton, the closure re-measure on the flip head, the record doc, the
  #83 follow-up comment. Deferred regardless of verdict: any statement
  beyond the subset, any perf tuning, any default visibility of the
  feature, and any hybrid change at all.
