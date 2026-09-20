# Rust-port fifth follow-up: the producer trigger fired before the producer was built

Report date: 2026-09-20. Mode: FOLLOW-UP to
`docs/reports/research-report-mypy-rs-rust-parity-followup-4-2026-09-20.md`
(the baseline, merged as PR #148 at `65e7b88ba`), which follows
`research-report-mypy-rs-rust-parity-followup-3-2026-09-20.md`,
`...-followup-2026-09-20.md`, `...-followup-2026-09-19.md` and the full
report `research-report-mypy-rs-rust-parity-2026-09-19.md`. Terms defined in
any prior report (wire, seam, defer, kernel-on/off, cold self-check, ownership
move, PyO3, the standalone skeleton, typeinfo read-closure, R, TypeInfo,
semantic-diff, support manifest, three-way partition, the hybrid) are used
bare; terms new since the baseline are defined at first use.

## 1. Orientation

Continues the baseline (`65e7b88ba`), which reported the class/member probe GO
at R = 0.0085 and asked when the standard-library producer starts and whether
traversal ownership does. The reader answered (the R = 0.0085 reply, verdicts
below), and every one of its items has a repo artifact this round: the
producer trigger was measured and fired, the perf gate is preregistered, and
traversal ownership was verified already true at module granularity. Eight
PRs merged. The gating obstacle is now concrete: what serves the four
standard-library primitive facts that four independent slices already read.

## 2. Verdicts on the reader's last advice

| reader advice | verdict | outcome and evidence |
| --- | --- | --- |
| Trigger the stdlib producer on semantic duplication (the first standard-library fact needed by two independent slices), not a corpus-size threshold; keep R >= 0.10 and closure > 200 as escalation thresholds | **tried; the trigger fired at its first reading** | The **duplication probe** (a script that reports which hand-built standard-library facts exist and how many manifest slices read each) was built as #155, `c403c9aaa`, record `docs/plans/2026-09-20-stdlib-duplication-probe.md:45`: `builtins.int` is read by 6 arms in 4 families, `bool`/`float`/`str` by 3 arms in 3 families each. Verdict in the record: "the trigger has already fired". The hand-maintained fact list is already the ad-hoc semantic database the trigger was designed to detect. |
| Producer shape: bulk at module granularity, sparse at semantic retention (parse `builtins.pyi` once, retain only facts reachable from requested names); escalate to a real Rust semantic-analysis pass only when the extractor must reproduce transformations | **adopted, written into the issue, untested** | #151's body carries the reader-fixed trigger, shape and the escalation list (MRO construction across source modules, state-dependent aliases, meaning-changing decorators, plugin hooks, multi-pass deferred resolution). No extractor code exists yet, so the shape is a design, not a result. |
| Start traversal ownership now; keep semantic analysis minimal until the producer shows which facts are needed | **accepted; verified already satisfied at module granularity** | #153/#154 (`33d1953a4`, `docs/remaining-migration-plan.md`, reader-verdicts section) record it with evidence: `Driver::check_main` (`crates/mypy-rs-skel/src/check.rs:202`) schedules every pass in Rust (name binding, class-model fixpoint, declaration binding, ordered module values, body sweep). The remaining traversal work is growing the pass surface, not flipping ownership. Semantic analysis stays deferred, per the advice. |
| First performance milestone: the first module checked end to end from a real producer with no hand-written records; three arms (upstream mypy; standalone warm; standalone cold); loose gate I < 2x, RSS < 2x, byte-identical diagnostics; wall-clock informational | **adopted, preregistered before any run** | #152, `docs/plans/2026-09-20-standalone-perf-gate-prereg.md` (PR #158, `270b543fb`). Arms A/B/C as advised; only arm B is gated, arm C is reported as the producer-amortization figure. The milestone module does not exist (the producer does not either), so nothing was measured. |
| Treat semantic-diff = 0 as a permanent invariant, not a test metric | **held; the round demonstrates the cost of holding it** | #159 (`95321806e`, closes #149) moved module-scope used-before-definition from silently-divergent to explicitly unsupported in the manifest (`module.value_used_before_definition`), shrinking the supported set rather than accepting a different answer; #161 files the real diagnostic as future work. |
| Grow another normal-language slice before anything exotic | **executed, in review** | The operators slice (#150, int/float/str binary operators, unary minus, augmented assignment) is PR #163 (`969b2caad`), open at writing, local battery green: 49 crate tests, codegen check clean, upstream gate at that head 13 cases: 2 pass, 11 unsupported, 0 semantic-diff, 0 unclassified; duplication probe re-ran with no fifth duplicated fact. |

## 3. What changed since the baseline (`65e7b88ba`)

- **Was "the producer question deferred, not answered by data" (baseline
  section 2), now answered by data**: the trigger fired, and #151 is the named
  next lane rather than a contingency.
- **Deferred resolution landed.** Was "in review" in the baseline (sections 3
  and 4); now merged: body-level forward references #126 (PR #145,
  `353ea4f9b`) and base/signature deferred resolution #138/#142 (PR #157,
  `6f57a7c16`), the latter after three fix rounds (section 4).
- **Correctness machinery extended.** The manifest on `main` is 39
  capabilities (16 supported, 23 unsupported); the operators PR takes it to 47
  (24 supported, 23 unsupported). The upstream corpus gate enforces the
  three-way partition at every run.
- **The perf gate went from an intention to a preregistration** with fixed
  arms, method and thresholds (#158).
- **Tracker.** Closed: #126, #138, #142, #144, #146, #149, #153. Filed:
  #160, #161, #162 (this round's parity gaps). Open: #135, #150 (PR #163),
  #151, #152 and the three new ones.

## 4. New failures

1. **Generic base arguments crashed instead of deferring.** Mechanism: base
   resolution tested whether each base name was bound, but not the type
   arguments inside a subscripted base. Break point: `class C(Box[D])` with
   `D` defined later exited 3 (`CheckError::Internal`) instead of deferring or
   rejecting, an internal-error escape outside the three-way partition. Fix
   (round 1 of #157): the readiness test recurses into subscript arguments;
   the self-referential `class C(Box[C])` now stalls into the cycle rejection
   (exit 2). Test `generic_base_argument_waits_for_later_class`
   (`crates/mypy-rs-skel/tests/class_slice.rs:1445`).
2. **Forward-base attribute ownership took three designs.** Mechanism: decide
   which pass owns an instance attribute inherited through a base defined
   later in the module. Break point: the retraction design (round 2) fixed the
   deferring direction and broke module-scope reads. Shipped design (round 3,
   `6f57a7c16`): collect class members in statement order first (so a later
   module value sees them, pass 3a), then clear every own instance variable
   (`clear_instance_vars`, `crates/mypy-rs-skel/src/check.rs:1266`) and
   re-collect in dependency order (`ordered_classes`,
   `crates/mypy-rs-skel/src/check.rs:79`, pass 3b). Two parity gaps remain
   open from it: #162 (a module value reading an attribute whose definer
   lands only in pass 3b: mypy accepts, the subset loud-rejects) and #160
   (the hierarchy-cycle rejection blames `remaining[0]`, a non-participant).
3. **Holding semantic-diff = 0 cost a supported arm.** Mechanism: the
   skeleton rejected module-scope used-before-definition with a different
   message than mypy's `used-before-def` diagnostic. Break point: leaving the
   arm in the SUPPORTED set would have been a silent divergence, the #93
   defect class; #159 moved it to UNSUPPORTED, and the fix (emit mypy's real
   diagnostic) is filed as #161.

Effort: #157 was three fix rounds plus a manifest conflict on rebase; the
other lanes were single rounds.

## 5. Conjecture ledger (changes only)

- **Bounded Python-free subset** (baseline medium, R = 0.0085): confidence
  up. The operators slice, the round's growth step, re-ran the duplication
  probe and produced no fifth duplicated fact, so slice growth has not yet
  grown the fact surface. Falsifier unchanged: the producer's first retained
  fact set could push the closure past 200.
- **Added: the four duplicated primitives are servable by a table.** The
  conjecture: `builtins.int/bool/float/str` can come from a bulk stub
  extractor with no semantic transformation, so the producer's first form is
  a lookup table, not an analyzer. Confidence medium. Falsified by the first
  fact whose retention needs cross-module MRO, a state-dependent alias, a
  meaning-changing decorator or a second semantic pass (the #151 escalation
  list). Cheapest test: retain the four primitives from `builtins.pyi` in
  #151 and require the existing slices to stay byte-identical.
- **Representation deletion** (report 2: low for the hybrid, medium for the
  standalone path): unchanged; the skeleton still has no wire to delete, and
  the hybrid is frozen as oracle.

## 6. New related work

None. The full report's citation list stands; no new citation was added.

## 7. Questions

Prior questions dispositioned: baseline Q1 (trigger metric and producer shape)
answered and encoded in #151; baseline Q2 (traversal and semantic ownership
now or later) answered, traversal verified owned, semantic analysis deferred;
baseline Q3 (first performance milestone) answered and preregistered in #152.

1. #151's escalation list names "MRO construction across source modules" as
   the first transformation that turns the extractor into a semantic
   analyzer. But within-file linearization is unavoidable for any stub class
   with bases, and `builtins.pyi` has them (`bool(int)`). Does the trigger
   mean only cross-module MRO, or does the first inherited stub record
   already count as the escalation? A wrong answer is detectable: it names a
   fact that either is or is not servable as a flat record. A useful answer
   names the first fact that must not be served by a table.
2. The preregistered gate compares median retired instructions for one
   module against upstream mypy at a 2x threshold. A single small module
   retires few instructions, so timer resolution and run-to-run variance are
   relatively large even at 2x. Should the measured unit be the whole
   producer-backed corpus plus its dependencies instead of one module, and
   does that change the "first no-record module" trigger? A useful answer
   names the unit and the variance it measured.
3. #162 is an open loud parity gap: mypy accepts a module value reading an
   attribute whose definer lands in pass 3b, the subset rejects. The two
   recorded directions are deferring the read until the definer's pass, or
   rejecting with a message naming the definer. A useful answer picks one,
   holds semantic-diff = 0, and names what each costs in pass complexity.

## 8. Carry-over

| still-open item | covered by |
| --- | --- |
| Stdlib producer #151, trigger fired, shape and escalation list fixed | this report section 2; `docs/plans/2026-09-20-stdlib-duplication-probe.md` |
| Standalone perf gate #152, preregistered, waiting on the first no-record module | this report section 2; `docs/plans/2026-09-20-standalone-perf-gate-prereg.md` |
| Operators slice #150, PR #163 open | this report sections 2 and 3 |
| Skeleton parity gaps #160, #161, #162; advisory batch #135 | this report sections 3 and 4 |
| Bounded-subset conjecture and its next falsifier | this report section 5 |
| Hybrid frozen as oracle; hybrid gap attribution (+82.4e9) never to be tabulated with standalone numbers | full report section 4; prereg section "Relationship to the old hybrid gate" |
| Failed hybrid levers (resolver snapshot, wire churn, identity memo, typeview, blob, subtype residency, leaf arena); deserialize-back dropped | full report section 5, follow-ups 1 and 2 |
| Related-work list and transfer verdicts | full report section 7 |
| Constraints: plugin-visible `__slots__` graphs, byte-identical diagnostics, 51 GB memory pool | full report section 8 |
| ADR-0008 standalone-path ownership, full-port definition of done, reader verdicts on roles and roadmap order | `docs/adrs/0008-standalone-path-ownership.md`, `docs/remaining-migration-plan.md`, epic #72 |
