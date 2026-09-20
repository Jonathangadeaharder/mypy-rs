# Rust-port fourth follow-up: the bounded subset survives its realistic test

Report date: 2026-09-20. Mode: FOLLOW-UP to
`docs/reports/research-report-mypy-rs-rust-parity-followup-3-2026-09-20.md`
(the baseline, committed as PR #113 at 07:54Z), which follows
`docs/reports/research-report-mypy-rs-rust-parity-followup-2026-09-20.md`,
`docs/reports/research-report-mypy-rs-rust-parity-followup-2026-09-19.md`
and the full report
`docs/reports/research-report-mypy-rs-rust-parity-2026-09-19.md`.
Terms defined in any prior report (wire, seam, defer, kernel-on/off, cold
self-check, ownership move, call-boundary optimization, PyO3, the standalone
skeleton, typeinfo read-closure, R, TypeInfo, the hybrid, ADR-0008) are used
bare here; terms new since the baseline are defined at first use.

## 1. Orientation

The reader fixed this round's experiment in its report-3 reply: grow the
skeleton by one realistic module of classes and member lookup (a class/member
slice: class definitions, inheritance, overrides, attributes and method calls,
with no async, decorated functions or PEP 695 syntax), re-run the typeinfo
read-closure probe, and build a standard-library TypeInfo producer only on a
NO-GO. The
probe ran GO: 15 distinct TypeInfos against the cold self-check's 1755, so
R = 0.0085 and the closure clause holds at 15 of 200. Eight PRs merged since
the baseline and the corpus is now checked end to end. The gating obstacle
moved off "does a bounded Python-free subset exist" onto "what produces the
standard-library class records the moment a slice needs one".

## 2. Verdicts on the reader's last advice

| reader advice | verdict | outcome and evidence |
| --- | --- | --- |
| Probe first: grow the skeleton by one realistic class/member module, build no `typeinfo-from-stubs` producer unless the probe fails; make it the last small-corpus experiment | **tried, GO on both preregistered clauses; advice fully executed** | Issue #114, record `docs/plans/2026-09-20-class-slice-probe.md:5-6`: 15 distinct TypeInfos, 56 fact-surface pairs, against 1755 (28,065), R = 15/1755 = 0.0085 < 0.10, closure 15 <= 200, diagnostics byte-identical to Python mypy. PR #117 at 08:23Z. The three NO-GO branches (bulk producer if R >= 0.10, bounded summaries if closure > 200, Rust semantic database if both) never fired, so the producer question is deferred, not answered by data. Slice implemented in #118/#128; a conditional control-flow slice followed in #136/#137. |
| Use both correctness gates: per-slice byte-identical differential, plus the upstream mypy corpus with an explicit unsupported allowlist | **adopted, implemented and in CI** | #115/#120 define the **support manifest** (the list of constructs the skeleton declares it handles) and the **three-way partition**: SUPPORTED must match Python mypy byte for byte, UNSUPPORTED must exit 2 naming the construct, a WRONG ANSWER is always a test failure. #119/#122 wire the upstream corpus as the global gate. Measured state: `13 cases: 2 pass, 11 unsupported, 0 semantic-diff, 0 unclassified`. The allowlist rule (`the test needs feature X, the checker reports X unsupported`) forbids the #93 hole at corpus scale. |
| Drop deserialize-back avoidance as superseded; do not spend a lane on it | **accepted, dropped, record was missing** | No experiment ran. The standalone path has no "deserialize back into Python" operation, so the lever has no consumer. It was recorded only in reports, not in a plan or ADR; this report is its close-out. |
| Text A roadmap (Rust owns `Type` via arena, then `Node`, traversal, semantic analysis, module state, CLI, plugin bridge; definition of done) | **unchanged from report 2, one ordering flip** | Report 2 adopted full-port-first and the definition of done (epic #72, `docs/remaining-migration-plan.md`). The one movement since: the arena is no longer the first slice. ADR-0008 moved representation ownership onto the Python-free skeleton path, and the skeleton needed no arena, no wire and no deletion experiment to check its corpus. |

## 3. What changed since 2026-09-20 (baseline 07:54Z)

- **Was "no realistic slice exists", now measured GO.** The baseline called the
  class/member probe the cheapest unrun test. It ran (#114, PR #117) and the
  bounded-subset conjecture strengthened from R = 0.0046 on a trivial corpus to
  R = 0.0085 on a realistic one.
- **Was "tracker empty" in the baseline, now six open.** #126, #135, #138,
  #142, #144, #146, all skeleton or tooling debt; see the carry-over table.
- **Corpus grew from one trivial module to three slices.** Class/member (#128,
  10:43Z), conditional control flow (#137, 11:26Z), and the hardening backlog
  (#139, 14:25Z) that cleared #121 and #123 through #132.
- **Correctness machinery exists.** Support manifest, three-way partition and
  the upstream corpus gate all landed this round (#120, #122).
- **Skeleton metric.** The upstream gate reports two passing cases and eleven
  explicit unsupported rejections, with the semantic-difference count fixed at
  zero, which is the invariant the reader asked to hold for the whole project.

## 4. New failures

1. **CodeQL classified a test fixture as production code.** Mechanism: the
   merge gate scans `crates/mypy-rs-skel/testdata/`. Break point: a deliberate
   `class D(A, A)` used to test duplicate-base diagnostics raised
   `py/inconsistent-mro` at `error` severity (alert 1444), failing the gate.
   The alert was dismissed as "used in tests"; the durable fix is #146, a
   `paths-ignore` for the testdata directory (PR #147, in review). Lesson for
   the migration metric: scanner findings on fixtures are false positives that
   mask real ones.
2. **A "medium advisory" was a silent divergence.** The round-3 review of the
   forward-reference work labelled it advisory. Verification showed real mypy
   rejects a subclass override whose base attribute is typed by a later module
   value, while the skeleton returned `Success` (exit 0). Break point:
   `check_classvar_overrides` ran in the early pass, before the re-collection
   pass that types late attributes. Fixed in `2f5c8a788` (PR #145). This is the
   #93 defect class, so it is blocking regardless of the reviewer's label.
3. **Editable install resolves `mypy` to a deleted worktree.** The shared
   `.venv` is a PEP 660 editable install whose `MAPPING` pins `mypy` to
   `worktrees/mypy-rs-118`, which no longer exists (verified at
   `.venv/lib/python3.13/site-packages/__editable___mypy_2_2_0_dev_15429..._finder.py:9`).
   Loud symptom: `ModuleNotFoundError` from any working directory outside a
   mypy tree. Silent symptom: the requested tree is shadowed by whichever tree
   last installed. Open as #144; fix is a coordinated reinstall, not yet done.
4. **Eager body checking rejected mypy-clean forward references.** Bodies were
   typed in statement order, so a body reading a module value assigned below it
   exited 2 where mypy accepts. Loud, but narrower than mypy for no measured
   reason. #126 fixed in PR #145; class bases and signature annotations are
   #138/#142, implemented on `fix/skel-deferred-resolution` and awaiting review.

## 5. Conjecture ledger (changes only)

- **Bounded Python-free subset (baseline confidence medium).** Confidence up:
  R = 0.0085 on a realistic slice, and the reader's own falsifier did not fire.
  New falsifier: the first slice that needs a standard-library TypeInfo
  (`builtins.int` timing, `typing` generics) either stays inside a nameable
  stubs subset or pushes R past 0.10, which would force the producer.
- **Representation deletion (carried from report 2).** Untested, and now
  cheaper to leave so: the skeleton checks its corpus with no wire to delete,
  so the conjecture's consumer is the hybrid, which is frozen as the oracle.
- **Deserialize-back avoidance (full report section 6 c2).** Killed by
  decision, not measurement: the reader directed the drop and no consumer
  exists. Confidence in its original 2-4e9 ceiling is unchanged and untested.

## 6. New related work

None. No citation has been added since the full report's list, which stands.

## 7. Questions

Prior questions, dispositioned. Report 3 Q1 (which slice grows the skeleton, and
what a NO-GO implies for the producer) answered and executed: the class/member
slice, GO, producer deferred. Report 3 Q2 (which intermediate correctness gate)
answered and built: both gates. Report 3 Q3 (run or drop deserialize-back)
answered: drop.

1. The bounded subset holds at R = 0.0085 with 15 TypeInfos for a class/member
   slice. The next slices (control flow landed, operators next) will eventually
   need standard-library facts (`builtins`, `typing`). At what measured point
   should the corpus stop carrying hand-built class records and start consuming
   a real producer, and should that producer be a bulk stubs table or a Rust
   semantic analysis pass? A useful answer names the trigger metric and the
   producer shape.
2. With eight slices landed and no wire in the skeleton, is the terminal
   architecture still "Python-free skeleton plus Python mypy as differential
   oracle", or should traversal and semantic-analysis ownership (the reader's
   Text A phases 3 and 4) start while the subset is still small enough to keep
   in one head? A useful answer names the milestone that flips work off the
   hybrid.
3. The reader's Text A wanted the performance gate to re-engage at deletion
   milestones. The skeleton has no deletion to measure yet. Is the right first
   performance milestone the first slice that reads a standard-library stub
   (where a producer's cost first appears), or the first module checked without
   any hand-written fixture? A useful answer names the milestone and the
   measurement.

## 8. Carry-over

| still-open item | covered by |
| --- | --- |
| Skeleton debt: forward references (#126, PR #145), deferred base/signature resolution (#138, #142), round-5 advisories (#135, 11 items) | issues; this report section 3 |
| Editable-install shadowing (#144), CodeQL testdata exclusion (#146, PR #147) | issues; this report section 4 |
| Support manifest, three-way partition, upstream corpus gate, their maintenance as slices grow | #115/#120, #119/#122 |
| Bounded-subset conjecture and its next falsifier (stdlib TypeInfo) | this report section 5, Q1 |
| Hybrid gap attribution, six buckets, +82.4e9 last measured before the remedy lanes | full report section 4 |
| Failed hybrid levers: resolver snapshot, wire churn, identity memo, typeview, blob, subtype residency, leaf arena | full report section 5, follow-ups section 4 |
| Deserialize-back avoidance: dropped, not measured | this report section 2 |
| Related-work list and transfer verdicts | full report section 7 |
| Constraints: plugin-visible `__slots__` graphs, byte-identical diagnostics, 51 GB memory pool, PyO3 boundary | full report section 8 |
| ADR-0008 standalone-path ownership and the full-port definition of done | docs/adrs/0008-standalone-path-ownership.md, epic #72 |
