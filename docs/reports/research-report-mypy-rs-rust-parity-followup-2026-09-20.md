# Rust-port second follow-up: ownership moved off the Python host

Report date: 2026-09-20. Mode: FOLLOW-UP to
`docs/reports/research-report-mypy-rs-rust-parity-followup-2026-09-19.md`
(the baseline) and `docs/reports/research-report-mypy-rs-rust-parity-2026-09-19.md`
(the full report). Terms defined in either prior report (wire, wire prep,
decode, seam, defer, kernel-on/off, cold self-check, ownership move,
call-boundary optimization, PyO3) are used bare here; terms new since the
baseline are defined at first use.

## 1. Orientation

The reader's advice (re-sent; its verdicts are outcome updates to what the
baseline already tracked, not a re-derivation) was: run one wire-deletion
ownership experiment on the subtype family, treat blob transport as the cheap
falsifier, and adopt full-port-first. That program has now run. The hybrid
parity question is closed: the ownership experiments are falsified, and the
gating question is no longer "can the hybrid reach parity" but "does a bounded
Python-free check path exist, and can a representation actually be deleted
there". ADR-0008 answers by building a standalone `mypy-rs` skeleton, a new
feature-gated executable that checks a fixed corpus with no `mypy` Python
import; Step 0 returned GO and the skeleton now exists in review.

## 2. Verdicts on the reader's advice

| reader advice | verdict | outcome and evidence |
| --- | --- | --- |
| Make the ownership experiment the subtype family (`rust_is_subtype`), stable IDs, delete that family's wire | **tried, NO-GO by the advice's own gate** | Issue #71. Family recurring wire cost B = 3.84e9. Gate (a), >=95% of wire deleted, cannot fire: 61.1% serialize-at-most-once ceiling. Gate (b), recover >=50% of B end-to-end, cannot fire: conversion ratio 0.14 to 0.29, hard population ceiling 0.43. The reader's conversion-efficiency falsifier fired. `docs/plans/2026-09-19-subtype-residency-step0.md`. |
| Prebuilt single-buffer blob on a hot 12-arg coded seam as the cheap falsifier | **tried, NO-GO** | Issue #70. Blob cuts the coded call 4,522 to 2,990 instructions (33.9%) with the buffer build excluded, under the >=70% gate; the deployable build-included form is -27.9%, because the Python buffer build (1,548) exceeds the whole marshaling saving. `docs/plans/2026-09-19-blob-marshal-falsifier.md`. |
| Prefer semantic-call batching over argument packing | **accepted, structurally blocked** | Results are consumed synchronously per question, so no outer operation can submit a multi-question packet. The arg-packing headroom is the real ceiling (0.98e9). |
| A Rust answer cache keyed on received bytes is a call-boundary optimization, worth testing | **accepted as classified, declined** | Correct classification; it cannot touch the Python-side dedup-key serialization it was proposed against, and the full port removes the boundary rather than caching across it. Superseded. |
| Adopt full-port-first; drop the per-phase net-positive requirement; gate performance at deletion milestones; write a definition of done | **adopted** | Epic #72 and the objective in `docs/remaining-migration-plan.md`. Definition of done written into #72. |
| Delete the representation instead of mirroring it; Phase 1 is Rust owns `Type` via an arena | **partially falsified, reordered** | The arena's own first slice inside the hybrid, minting a Rust ID at every `Type` construction, is NO-GO: 19.61 constructions per serialize-funnel appearance against a 1.0 gate (issue #81). ADR-0008 keeps the arena architecture, repositions it as the eventual plugin bridge, and starts representation deletion on the Python-free skeleton path instead. This is the one advice item where measurement changed the plan. |
| Premise: per-argument marshaling is ~392 instructions | **corrected** | #70 separates the conflated terms: the 12-arg-over-blob slope is ~139 instr/arg, not ~392. Ordinary arg packing has less headroom than the reply assumed. |
| Premise: 830 registered seams, 305 called, 532 never called | **stale figure, use not affected** | The seam-ledger planner count is 961; the epic's 830 is an earlier rounded figure (`docs/adrs/0008-standalone-path-ownership.md:115-118`). The skeleton adds no seam and retires none, so the count is unchanged. |

## 3. What changed since 2026-09-19

- **Was X in report N, now Y.** Baseline report, "what changed" list: ADR-0007 was proposed and the immutable-leaf arena flip was the planned first slice. Now ADR-0008 supersedes ADR-0007's *sequencing*: the next ownership flip is on the standalone path, and the hybrid arena flip is closed by measurement before any production code.
- **Third ownership experiment falsified.** #81 (leaf-arena amortization) NO-GO: 95.78% of leaf constructions never reach the serialize funnel, so minting at construction adds a hot-path crossing to delete no walk. Companion record `docs/plans/2026-09-19-type-arena-amortization-step0.md`.
- **The standalone skeleton is GO and built.** A typeinfo read-closure is the set of per-class records (`TypeInfo`: a class's name, bases, MRO and members) that a real check reads. #85 Step 0 measured it on the trivial corpus (one module: four annotated assignments, one annotated function, one deliberate error) at 8 distinct TypeInfos against the cold self-check's 1755, so the ratio R = 8/1755 = 0.0046 (< 0.10) and the absolute clause holds at 8 (<= 200). Record `docs/plans/2026-09-20-skeleton-bootstrap-step0.md`. Issue #91 and PR #92 build the feature-gated `mypy-rs` executable: byte-identical diagnostics against Python mypy on the fixed corpus, `otool`/`nm` show no libpython linkage, the seam inventory is unchanged, and zero bytes under `mypy/` changed.
- **Architecture decision written.** `docs/adrs/0008-standalone-path-ownership.md`: the next representation Rust owns is owned where Python is absent, so there is no second copy to keep coherent.
- **Call-boundary bookkeeping.** #84 unified the two wire-cache lookup paths; #89 decoded the remaining bare env gates.
- **Issue traffic.** Closed: #72 (epic), #81, #83, #85. Filed: #93, #94, #95 (skeleton hardening). Open: #88, #76, #77, #57, #42.
- **Seam count direction.** The production seam count is unchanged by the skeleton; the project's metric for a real port is that FFI seams should eventually fall, not rise (epic #72).

## 4. New failures

1. **Leaf-arena amortization (#81).** Mechanism: mint a Rust `TypeId` at every construction so later serialize-funnel appearances can skip their walk. Break point: the population, not the pricing. 661,412 of 690,538 constructions (95.78%) produce an object that is never serialized at the funnel on this build, so the mint arm pays ~19.6 crossings for every walk it could delete; the set ratio is 19.61 against a 1.0 gate, and `ErasedType`/`DeletedType` never appear at all. The priced delta is ~0.2e9 on a 430e9 build (~0.05%), reported but not the gate. Effort: Step-0 measurement only, no production representation written.

## 5. Conjecture ledger (changes only)

- Baseline-added conjecture, "representation deletion succeeds where shadow/mirror residency failed because it removes the Python representation": **mutated.** Its named cheapest test was exactly #81, and it failed *in the hybrid*. Deletion succeeds only on a path that never holds a Python copy; the skeleton is the first such path. Confidence low for any hybrid-resident form, medium for the standalone path.
- **Added conjecture.** A bounded Python-free check subset exists: the trivial corpus consults 8 TypeInfos, 0.46% of the self-check closure. Confidence medium. Falsified if growing the corpus toward realistic code pushes R to >= 0.10 or the closure past 200, which would mean no bounded subset exists and the honest next lane is a Rust typeinfo-from-stubs producer. Cheapest test: add one realistic module to the skeleton corpus and re-run the #85 probe.
- Prior report §6 c1 (boundary optimization cannot reach parity): unchanged, high; the blob leg is now measured closed.
- Prior report §6 c2 (deserialize-back avoidance): still open, untested.
- Prior report §6 c3 (`_is_subtype` dedup-key): declined and subsumed by the full-port decision.
- Prior report §6 c4 (ownership net-negative when landed incrementally): settled for two more forms (#54/#71/#81); the reader's wider claim is now carried by ADR-0008 rather than tested in the hybrid.

## 6. New related work

None. No citation has been added since the full report's list, which still stands.

## 7. Questions

Prior questions, dispositioned: full-report Q1 (terminal state vs bounded ownership step) answered, the bounded step ran and returned NO-GO, so the hybrid is terminal. Q2 (blob or buffer) answered, NO-GO. Q3 (byte-keyed cache) answered, boundary, declined. Q4 (next ownership kill experiment) answered, #71 then #81 ran it. Baseline Q1 (is the arena flip the right first slice, or does plugin visibility reorder it) answered, #81 killed the hybrid form and ADR-0008 reordered. Baseline Q2 (where does the perf gate re-engage) answered, #72/ADR-0008, at the standalone milestone, not now. Baseline Q3 (keep the hybrid shippable) answered, the hybrid stays the shippable artifact and correctness oracle, not the performance target.

1. The arena (Rust owns `Type`) is now positioned as the plugin-compatibility bridge for the standalone endgame, built late, while the skeleton grows first. Does Python-written plugin visibility of `Type` and `Node` force the arena to be designed before the skeleton corpus grows, or can the skeleton stay Python-free and accept a compatibility redesign later? A useful answer names the ordering constraint and the first plugin that would break it.
2. The skeleton's GO rests on R = 0.0046 measured on a trivial corpus (8 TypeInfos). What in a realistic mypy check makes the typeinfo read-closure grow, and is that growth bounded (a nameable stubs subset) or unbounded (the whole stdlib fact universe)? A useful answer names the growth mechanism.
3. Given three hybrid ownership experiments falsified, is the standalone skeleton the right first vertical cut, or should a cheaper falsifier (Rust semantic analysis from stubs for one module) prove the bounded subset before the skeleton builds the whole pipeline? A useful answer names the slice and the gate.

## 8. Carry-over

| still-open item | covered by |
| --- | --- |
| Deserialize-back avoidance lever (89k rebuilds, ceiling 2-4e9), untested | full report §4, §6 c2 |
| Gap attribution and the six buckets | full report §4 |
| Failed levers 1-2, identity memo, typeview rejection, seam retirement | full report §5 |
| Related-work list and its transfer verdicts | full report §7 |
| Constraints: plugin-visible `__slots__` graphs, byte-identical diagnostics, 51 GB pool, PyO3 boundary | full report §8 |
| Skeleton hardening follow-ups (#93 subset hard rejects, #94 stub prototypes, #95 Linux gate-2 port) | this report §3, issues |
| Remaining env gates (#88), bench harness (#76), blob parity (#77), CI cache layering (#57), stale-kernel remedy (#42) | issues |
