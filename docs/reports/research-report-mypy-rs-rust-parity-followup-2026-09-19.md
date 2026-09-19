# Rust-port follow-up: the hybrid lever set is closed, the full port is the objective

Report date: 2026-09-19. Mode: FOLLOW-UP to
`docs/reports/research-report-mypy-rs-rust-parity-2026-09-19.md` (same date).

## 1. Orientation

Continues `docs/reports/research-report-mypy-rs-rust-parity-2026-09-19.md`
(2026-09-19), which asked whether a Rust port of the mypy type checker can be
made net-faster than its Python host, and whether "Rust kernel, Python host" is
the terminal state. The reader answered: run one bounded wire-deletion
ownership experiment, then re-frame the project as a full Rust port. That
experiment has now been run and returned NO-GO. The full-port objective is
adopted (epic #72) and its first architecture decision is written (ADR-0007).
Asked of the reader now: whether the chosen first slice and milestone order are
right.

## 2. Verdicts on the reader's last advice

| reader advice | verdict | outcome and evidence |
| --- | --- | --- |
| Make the ownership experiment the subtype/type-graph family (`rust_is_subtype`), not `Instance` storage | **tried, NO-GO** | Issue #71. Step-0 battery: the family's current recurring wire cost B = 3.84e9 instructions. Gate 1 (>=95% of family wire deleted) cannot fire: 84.2% per repeat appearance / 75.2% per pair served, and a serialize-at-most-once ceiling of 61.1% of total family wire. Gate 2 ((I0-I1)/B >= 0.5) cannot fire: projected ratio 0.14-0.29, with a hard population-bound ceiling of 0.43. Record `docs/plans/2026-09-19-subtype-residency-step0.md` (branch `perf/subtype-residency-step0`, PR #78, open at writing). |
| Test prebuilt single-buffer (blob) transport on a hot 12-argument coded seam as a cheap falsifier | **tried, NO-GO** | Issue #70. Blob cuts the coded call 4,522.4 -> 2,989.8 instructions (33.9%) with the buffer build excluded, against a >=70% gate. The deployable build-included form is net negative: 4,522.4 -> 5,787.2 (-27.9%), because the Python buffer build alone is 1,547.8, more than the whole marshaling saving. Global projected ceiling 0.98e9 instructions, under the 1e9 gate leg, and unrealizable. Record `docs/plans/2026-09-19-blob-marshal-falsifier.md` (#75, `be76dd748`). |
| Prefer semantic-call batching over argument packing | **declined, structurally blocked** | Confirmed by the reader's own caveat: results are consumed synchronously per question, so no outer operation can submit a multi-question packet (`mypy/subtypes.py:958-994`). #70's 0.98e9 ceiling is therefore the real arg-packing headroom. |
| A Rust answer cache keyed on received bytes is a call-boundary optimization, worth testing | **not tried, declined** | It cannot touch the Python-side dedup-key serialization it was proposed against: that cost is 0.47s consumed plus 0.24s unconsumed per build, and Python must build the bytes before Rust can look them up. The reader's own classification (representation lifetime stays in Python) makes it a boundary optimization, not ownership. Superseded by the full-port decision, which removes the boundary rather than caching across it. |
| Adopt full-port-first; drop the per-phase net-positive requirement; gate performance at deletion milestones | **adopted** | Epic #72 and the objective section of `docs/remaining-migration-plan.md` (PR #73). A "definition of done" for mypy-rs 1.0 is written into #72. First slice: ADR-0007 and `docs/plans/2026-09-19-type-owner-inversion-brief.md` (PR #79, `3f81b9f7f`). |
| (premise used in advice) per-argument marshaling is ~392 instructions | **corrected** | #70 separates the terms the earlier microbenchmark conflated: the 12-arg-over-2-arg-blob slope is ~139 instr/arg, not ~392. Ordinary arg packing has less headroom than the prior report implied. Record `docs/plans/2026-09-19-blob-marshal-falsifier.md`. |

## 3. What changed since 2026-09-19

- The "highest-value remaining boundary experiment" (prior report §6 conjecture 1
  and question 2) is closed. Blob transport is NO-GO, not open.
- The incremental ownership route for the subtype family (prior report §6
  conjecture 4, confidence medium) is closed on population arithmetic: the
  pre-registered test fired NO-GO with no production code written.
- The marshaling attribution moved from ~392 to ~139 instr/arg. The prior
  report's per-argument figure overstated the plain argument-packing leg.
- The objective flipped. "Parity of the hybrid is existential" (prior report §2
  and §1 close) was already corrected in the prior report's addendum; epic #72
  now makes full-port-first canonical and moves the performance gate to deletion
  milestones.
- ADR-0007 is proposed: a Rust type arena (`TypeId -> TypeRecord`) with Python
  `Type` objects becoming facades rather than canonical objects.
- Two follow-ups filed during the measurement: #76 (bench-harness edge cases)
  and #77 (blob/coded parity drift).

## 4. New failures

1. **Blob / single-buffer transport.** Mechanism: build one buffer in Python,
   make one PyO3 call, avoid per-argument extraction. Break point: the Python
   buffer build costs more than the entire per-argument saving it enables, so the
   deployable form is -27.9%; the floor is wire prep, not the crossing. Effort:
   one measurement lane, one day.
2. **Subtype residency with stable IDs (wire elimination).** Mechanism:
   serialize each participating operand at most once, pass stable Rust handles
   for that family, delete the family's wire. Break point: 38.9% of classified
   operand appearances are first-sight distinct objects (116,655 of 299,558), so
   serialize-at-most-once deletes only 61.1% of family wire, under the 95%
   requirement; and the end-to-end conversion ratio is bounded at 0.43 by the
   class-A population (59,377 pairs), under the 0.5 requirement. Effort: Step-0
   measurement only; no production representation was written.

## 5. Conjecture ledger (changes only)

- Prior §6 conjecture 1 (boundary optimization cannot reach parity): was
  confidence high, now measured rather than conjectured, with the packing leg at
  <=0.98e9.
- Prior §6 conjecture 4 (ownership is net-negative when landed incrementally):
  was medium. For `rust_is_subtype` it is now settled. The wider claim is not
  falsified: the tested form kept both representations alive, which is what the
  full port removes.
- **Added conjecture.** Representation deletion succeeds where shadow/mirror
  residency failed, because it removes the Python representation instead of
  keeping both. Confidence medium. Falsified if the ADR-0007 first slice
  (immutable leaf family, default-off gate) fails correctness parity in both gate
  states, or if gate-on cannot make that family's Python `__slots__` and wire
  path structurally unreachable while reporting total instructions and RSS.
  Cheapest next test is that slice. Cost so far: design only.

## 6. New related work

None delivered. No citation has been added since the prior report's list, which
still stands.

## 7. Questions

Prior questions, dispositioned: Q1 (terminal state vs bounded ownership step)
answered, the bounded experiment ran and returned NO-GO, so the hybrid is
terminal for this project. Q2 (blob or buffer highest value) answered, NO-GO. Q3
(byte-keyed cache boundary or ownership) answered, boundary, declined. Q4 (next
ownership kill experiment) answered, Step-0 ran it.

1. Is the immutable-leaf-family arena flip (ADR-0007 first slice) the right
   first vertical slice, or does Python plugin visibility of `Type` objects force
   a different first order? A useful answer names the first family and the
   correctness gate.
2. On the full-port path, should the performance gate re-engage at traversal
   ownership (roadmap phase 3, a Rust driver that internally performs many
   operations) or only at the standalone-executable milestone (phase 5, checking
   with no `mypy` Python modules imported)? A useful answer names the milestone
   and the measurement.
3. Is there any remaining reason to keep the hybrid as a shippable artifact, or
   is all future work on the standalone binary? A useful answer is yes or no with
   the condition.

## 8. Carry-over

| still-open item | covered by |
| --- | --- |
| Deserialize-back avoidance lever (89k rebuilds, ceiling 2-4e9), untested | prior report §4, §6 c2 |
| `_is_subtype` dedup-key lever (0.47s + 0.24s), now subsumed by #71 | prior report §6 c3 |
| Gap attribution and the six buckets | prior report §4 |
| Failed levers 1-2, identity memo, typeview rejection, seam retirement | prior report §5 |
| Related-work list and its transfer verdicts | prior report §7 |
| Constraints: plugin-visible `__slots__` graphs, byte-identical diagnostics, 51 GB pool, PyO3 boundary | prior report §8 |
