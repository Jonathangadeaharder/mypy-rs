# Rust-port third follow-up: the tracker is clear, the skeleton is the only front

Report date: 2026-09-20. Mode: FOLLOW-UP to
`docs/reports/research-report-mypy-rs-rust-parity-followup-2026-09-20.md`
(the baseline), which follows
`docs/reports/research-report-mypy-rs-rust-parity-followup-2026-09-19.md`
and the full report
`docs/reports/research-report-mypy-rs-rust-parity-2026-09-19.md`.
Terms defined in any prior report (wire, seam, defer, kernel-on/off, cold
self-check, ownership move, call-boundary optimization, PyO3, skeleton,
typeinfo read-closure, R, TypeId, ADR-0008) are used bare; terms new since
the baseline are defined at first use.

## 1. Orientation

Since the baseline (commit `d9b07e181`), every open issue in the tracker has
been resolved and merged: 13 commits and 13 PRs in the range
`d9b07e181..e6dc6f513`, and `gh issue list --state open` now returns empty
(verified 2026-09-20). The skeleton, the feature-gated standalone `mypy-rs`
executable, landed. No new reader advice arrived, and no new performance
measurement was taken: the round cleared correctness, test and CI debt that
the full-port program had accumulated. The gating obstacle is unchanged from
the baseline: whether the skeleton's bounded-subset GO survives realistic
code, whose cheapest test (grow the corpus by one realistic module, re-run
the typeinfo read-closure probe) is still unrun.

## 2. Verdicts on the reader's last advice

No reader reply arrived after the baseline. The baseline's verdict table
(section 2 there) is the standing verdict record; every row's state is
unchanged, because nothing in `d9b07e181..e6dc6f513` touched a falsified
route, the adopted full-port objective, or the deletion-milestone performance
gate. The adopted items advanced only in execution: the skeleton exists, and
the debt the program had deferred is cleared.

## 3. What changed since 2026-09-20

- **Skeleton landed.** Was "in review" at baseline, now merged: PR #92,
  `74c0a0ea0`, closed #91 (2026-09-20T00:19:55Z). The executable checks the
  fixed corpus with no `mypy` Python module imported and no libpython
  linkage (`otool`/`nm`), diagnostics byte-identical to Python mypy on both
  corpus arms.
- **Subset hard-rejects.** The skeleton subset now rejects constructs whose
  semantics diverge from mypy instead of accepting them silently: async,
  decorated and PEP 695 generic functions. PR #102, `a369da5b2`, closed #93.
  This hardens the GO evidence: a partial checker can no longer pass the
  differential by quietly mis-checking unsupported syntax.
- **Struct-layout claim refuted.** The repo's automated code-review bot
  asserted `PyLongObject` is 40 bytes on CPython 3.12+, which would have
  mis-sized the skeleton's Python C API stub declarations. A compiled
  `sizeof` probe across CPython 3.9.25, 3.10.20, 3.11.15, 3.12.13, 3.13.14
  and 3.14.5 measured `PyLongObject` = 32 bytes on every version (3.12 folded
  the variable-length head into `lv_tag`), and `PyTypeObject` = 408 on 3.11
  and older vs 416 on 3.12 and newer; the stubs size for 416, the larger
  layout across the supported version range. PR #106, `be93a498d`, closed #94.
- **Stale-kernel remedy complete.** The stale-kernel remedy is the fetch path
  that repairs a missing gated seam binding loudly instead of silently
  deferring: a stale extension binary re-registers its seams or fails, never
  quietly falls back. It now covers every gated fetch in `join.py` (7),
  `meet.py` (6) and `solve.py`, and the decision (#108) is loud everywhere,
  safe because the kernel crate registers every seam unconditionally (no
  feature gating). PRs #99 and #107.
- **Tracker emptied.** All baseline carry-over issues closed: #88 (bare env
  gates, PR #97), #42 (remedy coverage, PR #99), #77 (blob parity drift, PR
  #100), #93 (subset, PR #102), #94 (stubs, PR #106), #95 (hardening, PR
  #109), #57 (CI cache layering, PR #103), #76 (bench harness, PR #105),
  plus #98, #101, #104 (filed and closed in-round by PRs #107, #111, #110).
- **Validity caveat.** The hybrid's +82.4e9 gap figure (full report section 4)
  was not re-measured; the remedy lanes touched production fetch paths after
  the last paired run. No standing decision reads that number, since the
  hybrid is now the correctness oracle, not the performance target.

## 4. New failures

1. **Kernelless regression, introduced and fixed.** PR #111 made the
   stale-kernel suite's positive controls falsifiable by decoding stub bytes.
   Break point: the test helper activated a stub kernel by patching only
   `_WriteBuffer`, while production binds three globals (`_WriteBuffer`,
   `_ReadBuffer`, `_read_type`) together with the kernel import, so in a
   kernelless environment (the type_kernel extension absent, every gated
   seam deferring) the decode raised `TypeError: 'NoneType' object is not
   callable` at `mypy/subtypes.py:351`: 6 passed at the pre-#111 head,
   1 failed at the merged head. Fix in PR #112, `e6dc6f513`, patches all
   three bindings; verified post-merge at 8 passed, 1087 skipped, 27
   subtests. Effort: small; the invariant (the bindings move with the
   import) is now encoded in the helper.
2. **Vacuous bench gate.** Hardening the microbench harness (#76) found its
   own success check tautological: `ser_hit or before >= 1` could never
   fail, and a default cache tuple carried the wrong shape. A structural
   zero, a check that can never fail, the exact defect class the repo's
   evidence rules name as worse than no check. Fix in PR #105, `bde54575e`.

## 5. Conjecture ledger (changes only)

- Bounded Python-free subset (baseline section 5; R = 0.0046, confidence
  medium): measured value unchanged, no new probe ran. Validity strengthened:
  the subset now hard-rejects divergent constructs, so the byte-identical
  differential can no longer pass on silently mis-checked syntax. Cheapest
  test unchanged: one realistic module into the skeleton corpus, re-run the
  closure probe; falsified if R >= 0.10 or the closure passes 200.
- Hybrid-resident representation deletion (baseline section 5, low
  confidence): unchanged; no hybrid experiment ran.
- Deserialize-back avoidance (full report section 6 c2): unchanged, still the
  only untested hybrid lever (89k rebuilds, ceiling 2-4e9).
- No conjectures added or killed this round.

## 6. New related work

None. The struct-layout probe is artifact evidence from CPython's own memory
layout, not a citation; the full report's list stands.

## 7. Questions

Prior questions, dispositioned: baseline Q1 (plugin visibility vs arena-bridge
ordering) unanswered, carried below. Baseline Q2 (what makes the closure grow)
folds into Q1's gate. Baseline Q3 (skeleton vs a cheaper falsifier) superseded
by Q1: the skeleton exists, so the open question is which slice grows it.

1. The tracker is empty; the next slice must be created. Grow the skeleton
   corpus by one realistic module and re-run the typeinfo read-closure probe
   (the pre-registered falsifier: R >= 0.10 or closure > 200 kills the
   bounded-subset conjecture), or go straight to the Rust
   typeinfo-from-stubs producer the conjecture names as fallback? A useful
   answer names the slice, the gate, and what a NO-GO on the probe implies
   for the producer's design.
2. As the skeleton grows, what is the intermediate correctness gate: per-slice
   byte-identical differential against Python mypy, or the upstream mypy test
   corpus with an explicit known-unsupported allowlist? A wrong answer is
   detectable: the gate must fail when the skeleton mis-checks something it
   claims to support, the #93 class of hole.
3. Deserialize-back avoidance is the last untested hybrid lever (full report
   section 6 c2; pre-registered gate: under 30% of rebuild inputs repeat
   within a build, drop). Run that falsifier before the hybrid freezes into
   its correctness-oracle role, or drop it as subsumed by the full port? A
   useful answer is run or drop, with the condition.

## 8. Carry-over

| still-open item | covered by |
| --- | --- |
| Plugin visibility vs arena-bridge ordering (baseline Q1) | baseline §7 Q1 |
| Deserialize-back lever, pending Q3 | full report §4, §6 c2 |
| Gap attribution, six buckets, +82.4e9 last measured before the remedy lanes | full report §4 |
| Failed levers: resolver snapshot, wire churn, identity memo, typeview, blob, subtype residency, leaf arena | full report §5, follow-ups §4 |
| Related-work list and transfer verdicts | full report §7 |
| Constraints: plugin-visible `__slots__` graphs, byte-identical diagnostics, 51 GB pool, PyO3 boundary | full report §8 |
| Bounded-subset conjecture and its probe | baseline §5, this report Q1 |
| ADR-0008 skeleton growth record | docs/adrs/0008-standalone-path-ownership.md |
