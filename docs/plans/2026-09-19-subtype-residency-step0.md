# Subtype residency Step 0: B battery and operand-level probe (#71)

Status: measurement only, no production change. Verdict: **NO-GO** under the
pre-registered gate — both gate-1 readings and gate 2 cannot fire as written.
This record closes the Step-0 falsifier; per #71's on-failure clause the
ownership route closes unless a qualitatively different architecture is
proposed. Companion: issue #71; design brief
`docs/plans/2026-09-19-subtype-residency-brief.md` (sections 3-5 are the
pre-registered definitions used below); parent attribution
`docs/plans/2026-09-19-gap-attribution.md` (#61); probe history
`docs/plans/2026-09-19-perf-subtype-identity-probe.md` (#58).

- Head: `origin/main` = `631c12a45`, worktree
  `worktrees/mypy-rs-residency-step0` (branch `perf/subtype-residency-step0`);
  no Rust change, Python probe + driver + stub declarations only.
- Extensions built from this tree into the private scratch
  `/private/tmp/mypy-rs-local-typekernel-step0/` (codesigned; the shared
  typekernel scratch belongs to sibling lane #70 and was never touched);
  shared `/private/tmp/mypy-rs-local-{ast,resolver}` on PYTHONPATH.
  `scripts/assert_worktree_import.py` exit 0 before every counted run.
- Corpus: cold self-check, `mypy_self_check.ini -n0 --no-incremental
  -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`), 378 files,
  "Success: no issues found in 378 source files" on every counted run.
- Evidence: `/private/tmp/residency-step0/` (probe runs 1-2, post-fix
  re-measurement run 3, ledger, baseline, microbench table).
- Load during counted runs: load1 39-54 for runs 1-2 (sibling lanes
  active), ~12 for run 3 (idle machine); instructions are the
  load-invariant currency, wall seconds recorded only.

## The pre-registered gates (from #71, verbatim in brief section 5)

1. >=95% of repeat serialization/decode for the family disappears.
2. (I0 - I1) / B >= 0.5 on the cold self-check.
Brief section 3 records the two readings of gate 1 (repeat-class wire vs
total family wire) and the on-failure clause; both were fixed before this
lane ran and are not reinterpreted below.

## B battery

B = the family's recurring end-to-end instruction cost per cold self-check
(brief section 4: serialize prep at the two `_is_subtype` sites including
dedup-key builds and dict upkeep, crossing marshal, family-attributed decode;
deser-back B4 = 0, i8 return).

### B1 route (a): ledger clocks x blend rate

Wire-prep ledger on this head (`scripts/measure_wire_prep.py`, family sites
`subtypes.py:1171` / `:1172`, the two `_serialize_type` calls in the native
block; consumed + unconsumed, matching the #61 methodology):

| site | consumed | unconsumed | entries (blob-keyed) |
| --- | --- | --- | --- |
| subtypes.py:1171 | 0.250 s | 0.120 s | 148,958 |
| subtypes.py:1172 | 0.161 s | 0.093 s | 148,957 |
| **total** | **0.411 s** | **0.213 s** | 0.624 s |

B1a = 0.624 s x 5.9e9 instr/s (the #61 ledger blend: 3.45 s <-> ~20.6e9;
provenance is the attribution head, load1 35-52) = **3.68e9**.
The ledger "entries" are blob-keyed (a wire-cache hit re-serving a blob
re-accumulates onto one key), so they are not comparable to the probe's
per-call appearance counts below; the site durations cover every call.

### B1 route (b): event model with the family's own hit/miss split

New probe counters measure the family's serialize-outcome split per operand
appearance (prediction replicates `_serialize_type`'s decision order
exactly; the 21 `ser_failed` entries' 42 predicted appearances are excluded
from the event model): hit 180,623, builtin shortcut 29,429, miss 89,506 of
299,558 classified appearances (run 1; run 2 within 0.1%; the post-fix
re-measurement below confirms the split). Microbench
re-derived on this head, private scratch: ser_hit 1,815.3, ser_miss_any
15,265.2, ser_miss_instance 16,071.9 instr/call (all within 0.5-2.2% of the
#61 table). The builtin shortcut has no bench; folded as hit (lower bound)
and as miss (upper bound):

- B1b = (180,623 + 29,429) x 1,815.3 + 89,506 x 15,265.2 = **1.75e9**
- B1b upper = 180,623 x 1,815.3 + (89,506 + 29,429) x 16,071.9 = **2.24e9**

### B1 route disagreement: the pre-registered falsifier fires

|3.68e9 - 1.99e9| / 3.68e9 = 46% >> 15%. The brief lists this disagreement as
a falsifier of the B estimate, so it is reported, not averaged away. Cause,
identified rather than assumed: route (b) prices only the serialize leg,
while B1's definition — and the site clocks — include the dedup-key builds,
bytes hashing, ctx-key/tuple construction and `_subtype_answers` dict upkeep
booked at the sites. The ~1.6-2.1e9 residual is that non-serialize site
work, and its magnitude is corroborated independently: the e2e repeat
microbench (28,090.9 instr for one repeat-path `_is_subtype` call) minus its
two hit serialize legs (2 x 1,815) minus the coded crossing (~4,503) leaves
~20k instr of key/upkeep per repeat-path call, the same work the residual
prices. Route (a) is the full-scope B1 per the definition; route (b) is
reported as the serialize-leg floor.

### B2, B3, B4

- B2 (crossing marshal): coded_calls 22,652 x 4,502.7 (12-arg coded-call
  bench, this head) = **0.102e9**.
- B3 (family-attributed decode): decode-window deltas around each coded
  call: 320,897 decode nodes x ~185 instr/node (#63 sample-implied) =
  **0.059e9**. Caveat, stated: the window attributes all decode inside the
  coded call to the family; fam_decode_calls = 61,036 = 2.69 x coded_calls
  (consult re-entrancy is caught inside the windows, quantified by exactly
  this ratio, never read as zero).
- B4 (deser-back) = **0** for this family: the coded entry returns i8.

### B, and the repeat-class sub-B the gates act on

- B (full scope, route a) = 3.68 + 0.10 + 0.06 = **3.84e9** — inside the
  brief's expected band 3.5-5.5e9, ~4.7% of the +82.4e9 kernel gap.
- B (serialize-leg scope, route b) = 1.91-2.40e9, reported for the spread.
- Repeat-class sub-B (class A entries only: both operands repeats; they are
  answer-cache content hits, so their B2/B3 share is zero): the addressable
  cost is the served population x the full repeat-path site cost
  (~24.6-28.1k instr/call: route-a per-entry average and the e2e bench) =
  0.77-1.25e9, i.e. at most ~33% of B. This is what the gates act on.

## Probe populations (two counted runs; spread <= 0.1%)

| counter | run 1 | run 2 |
| --- | --- | --- |
| native-path entries | 149,800 | 149,786 |
| operand appearances | 299,600 | 299,572 |
| classified appearances (excl. 2 x ser_failed) | 299,558 | 299,534 |
| op_first_seen | 107,685 | 107,617 |
| op_recycled (id reused by a new object) | 8,970 | 9,021 |
| **D = distinct operand objects** | **116,655** | **116,638** |
| op_repeat (byte-verified same-object) | 182,903 | 182,896 |
| op_ser hit / builtin / miss | 180,623 / 29,429 / 89,548 | 180,610 / 29,416 / 89,546 |
| op_repeat_servable | 153,939 | 153,929 |
| content hits | 127,148 | 127,136 |
| class A pairs (both operands repeats) | 59,377 | 59,367 |
| class B pairs (>=1 fresh operand) | 67,771 | 67,769 |
| identity hits (whole-pair same-object repeats) | 41,735 | 41,713 |
| memoable, both operands servable | 31,376 | 31,353 |
| coded calls / defers | 22,652 / 87 | 22,650 / 85 |
| ser_failed entries | 21 | 19 |
| op_class_anomaly / overflows | 0 / 0 | 0 / 0 |

D share: 116,655 / 299,558 = 38.94% of classified appearances are
first-sight (or recycled-id) objects. Repeat share: 61.06%.
Repeat classes: plain 99.68%, fingerprinted 0.25%, tvar_root 0.07%,
prefixup 0, cache_off 0 — the F3 store-class exclusions barely fire, but the
servability predicate (F3-cache hit at arrival) is what excludes.

**Exclusion rate among repeats = 1 - 153,939/182,903 = 15.84%.** Only 84.2%
of repeat appearances would be served by the residency table at all.

### Post-fix re-measurement (runs 3-4)

The OCR passes on the measurement PR reclassified probe defects (the
anomaly-class counter path that would raise `KeyError` mid-run, the decode
window left open by an exception escaping the coded call, the missing
mirror-serve ser class, the failed-entry appearances left outside operand
accounting, and the pre-warm right-operand ser prediction, which
mislabeled the second serialization of a same-object pair; a later round
also had the operand classifier validate stored fingerprints, bucketing
stale ones as `fp_stale` — a non-servable class). All were fixed and two
post-fix counted runs were taken (self-check success, exit 0, no evidence
refusals, wire-phase mode 1, classification sum reconciled:
`op_unclassified` 38 = 2 x `ser_failed` 19). The fixes' effect on this
corpus sits inside the run-1/run-2 spread, so every number above stands:

| counter | run 3 (post-fix) | run 1-2 band |
| --- | --- | --- |
| op_ser hit / builtin / miss / mirror | 180,602 / 29,418 / 89,542 / 0 | ~180.6k / ~29.4k / ~89.5k / n/a |
| op_repeat_servable (exclusion 15.83%) | 153,927 | 153,929-153,939 |
| memoable_servable_both (75.18% of identity hits) | 31,365 | 31,353-31,376 |
| B1b / B1b upper (event model re-derived on run 3) | 1.75e9 / 2.24e9 | 1.75e9 / 2.24e9 |
| D / class A pairs / coded calls | 116,659 / 59,366 / 22,652 | unchanged by construction |
| op_first_fp_stale / op_repeat_fp_stale (run 4) | 0 / 0 | the stale-fp class never fires |

A sixth review round reclassified the probe's decode-window guard: the
windows == coded-calls check was a near-tautology (every coded call closes
exactly one window), so a skipped window never refused on its own. The
guard now refuses on `fam_window_skipped` directly and keeps the
windows/coded-calls invariant for the `wire_open` escape path. Run 5
(post-fix): exit 0, mode 1, `fam_window_skipped` 0, op_ser
180,597/29,418/89,549, op_repeat_servable 153,933, memoable 31,355, D
116,657 — inside the run 1-4 band, no number above moves.

The self-pair bias the pre-warm prediction carried is therefore
negligible here: reclassifying a same-object pair's second serialization
from pre-warm miss to hit moves the split by less than the run-to-run
noise. The verdict's hard population bound (class-A ceiling) reads the
pair-level repeat populations, which the ser-class timing never touched.

## Gate arithmetic

**Gate 1, reading 1 (>=95% of repeat-class wire disappears).** The
operationalization (brief section 3) needs repeat-class entries served
without serialization / baseline repeat-class entries >= 0.95. Measured:
per repeat appearance the table serves 84.2% (exclusion 15.84% >> the 5%
headroom); per whole-pair repeat both operands are servable in only
31,376/41,735 = 75.2% of cases; even against the full class-A population
the projected serve rate is 44,640/59,377 = 75.2%. **Cannot fire.**

**Gate 1, reading 2 (>=95% of total family wire disappears).** Serialize-at-
most-once per object makes deletion = 1 - D/appearances = 1 - 116,655/299,600
= **61.06%** < 95%. D is large, as the entry-cost mix predicted: 67,771
content-hit pairs (53% of content hits) have at least one fresh operand —
the kernel's own deser-back churn manufactures byte-equal fresh objects, and
eliminating their first walk is deser-back avoidance (attribution lever 3),
a different lever with its own falsifier. **Cannot fire.**

**Gate 2 ((I0 - I1)/B >= 0.5).** Served lookups need both operands resident
(first sights cannot be served in any architecture that leaves Python
canonical) and the answer present: bounded by the 31,376 both-servable
whole-pair repeats (floor; the brief's 41.7k floor shrinks to this once
operand-level exclusion applies) and ~44,640 (ceiling: 59,377 class-A pairs
x the measured 75.2% pair-servable rate). Savings at ~23-27k instr per
served lookup (brief cost model; the e2e repeat bench re-derived at 28,090.9
bounds it) minus 0.1-0.2e9 new costs gives I0 - I1 = 0.52-1.11e9.
Ratio against full-scope B = 3.84e9: **0.135 - 0.289 < 0.5.** Scope-consistent
serialize-leg ratio (serialize-leg savings / B1b): 0.05-0.09. The ratio is
below the threshold under every scope reading.

**The decisive number is the population itself:** reaching 0.5 x B = 1.92e9
needs >= 71,000 served lookups at the most generous 27k saved each. The
class-A ceiling is 59,377 pairs. Even with zero exclusions, zero new costs
and every class-A pair served at the full e2e cost (59,377 x 28,090 =
1.67e9), the ratio tops at 0.43 < 0.5. No measurement refinement rescues it;
the repeat-class population is too small to carry gate 2.

## Verdict

**NO-GO.** Gate 1 cannot fire under either reading (84.2%/75.2% vs 95%;
61.1% vs 95%). Gate 2 cannot fire: the projected ratio is 0.14-0.29, with a
hard population-bound ceiling of 0.43, all below 0.5. The falsifier did
what it was designed to do: it killed the flip lane before a line of
production code was written, and it says where the mass actually is — 38.9%
first-sight operands and 53% fresh-operand content hits point at deser-back
churn (attribution lever 3), not boundary-work duplication, as the lever
that would have to move. Per #71's on-failure clause the ownership route is
closed unless a qualitatively different architecture is proposed.

## What ships

The instrumentation this record rests on, all default-off and
evidence-critical:

- the operand-level extension of the identity probe in `mypy/subtypes.py`
  (env gate `MYPY_SUBTYPE_IDENTITY_PROBE` unchanged, default off): operand
  distinctness against recycling, storable-class split, the family's own
  serialize hit/miss split (mirror serves and failed-entry appearances are
  their own classes, so a mirror-on run is detectable instead of misread
  and the appearance accounting always reconciles), coded-call count, and
  decode-window attribution with skip/inconsistency accounting;
- `misc/residency_step0_probe.py`, the run driver (reports on every exit
  path, refuses its own evidence on the pre-registered conditions,
  including an off wire-phase profile and a classification-sum mismatch,
  and fails loudly on a stale extension that predates the seam);
- `stubs/type_kernel_misc.pyi`: the `rust_wire_phase_*` declarations the
  self-check needs (the extension has exported them since #63);
- `misc/gap_attrib_microbench.py`: env-overridable scratch/output dirs and
  a `--only` filter, so a lane can re-derive the per-call table against its
  own private scratch without touching the #61 evidence defaults.

## Caveats, stated rather than hidden

- Route (a) transfers the #61 blend rate (5.9e9 instr/s, provenance:
  attribution head, load1 35-52); the 15% agreement gate was the transfer's
  validation and it failed, so both routes are reported with the cause
  identified (serialize-leg vs full-site scope) and an independent
  corroboration of the residual.
- The 15-30% lower family clocks vs the #61 head (0.624 s vs 0.711 s) are
  wall-clock site timings under different load; the instruction-denominated
  route (b) and the counter populations agree across runs to <= 0.1%, which
  is why counters, not site clocks, carry the verdict.
- The probe holds `_identity_seen` / `_operand_seen` and so pins object
  lifetimes: probe-on instructions (441.2e9) exceed the probe-off kernel-on
  baseline (430.5e9, this head) by 2.5%; probe-on runs are never used as
  instruction baselines, only as counter sources.
- `fam_decode_calls` = 2.69 x coded_calls quantifies consult re-entrancy
  inside the decode windows; B3 uses decode_nodes, whose family attribution
  is unaffected by the re-entrancy count but inherits the window's
  all-decode-inside-the-call assumption.
- The builtin-shortcut serialize class (9.8% of appearances) has no dedicated
  bench; B1b is reported as a fold-as-hit/fold-as-miss band.
- 21/19 ser_failed entries per run leave 42/38 appearances outside operand
  classification (D's base is stated per run; the probe now counts them as
  `op_unclassified` and the driver refuses on a classification-sum
  mismatch); the ledger's blob-keyed "entries" are a different accounting
  and never compared to probe counts.

## Reproduction

```bash
# extensions: build type_kernel into the private scratch (AGENTS.md order)
cargo rustc -p mypy-type-kernel --features extension-module --lib \
  --crate-type cdylib --release -- -C link-arg=-undefined -C link-arg=dynamic_lookup
cp target/release/libtype_kernel.dylib \
  /private/tmp/mypy-rs-local-typekernel-step0/type_kernel.cpython-313-darwin.so
codesign -f -s - /private/tmp/mypy-rs-local-typekernel-step0/*.so

export PYTHONPATH=/private/tmp/mypy-rs-local-typekernel-step0:\
/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver
# from the worktree root, before every counted run:
.venv/bin/python scripts/assert_worktree_import.py .

# counted probe runs (pool corpus-class)
MYPY_SUBTYPE_IDENTITY_PROBE=1 MYPY_NUM_WORKERS=0 \
  /usr/bin/time -l .venv/bin/python misc/residency_step0_probe.py

# family ledger clocks
MYPY_AUDIT_ARGS="-n0 --no-incremental -p mypy -p mypyc" MYPY_NUM_WORKERS=0 \
  /usr/bin/time -l .venv/bin/python scripts/measure_wire_prep.py

# per-call table on the private scratch
MYPY_MICROBENCH_TK_SO=/private/tmp/mypy-rs-local-typekernel-step0/\
type_kernel.cpython-313-darwin.so \
MYPY_MICROBENCH_OUT_DIR=/private/tmp/residency-step0 \
  .venv/bin/python misc/gap_attrib_microbench.py \
  --only ser_hit,ser_miss_any,ser_miss_instance,pyo3_13args,is_subtype_e2e_repeat_on

# probe-off baseline
MYPY_NUM_WORKERS=0 /usr/bin/time -l .venv/bin/python -m mypy \
  --config-file mypy_self_check.ini -n0 --no-incremental -p mypy -p mypyc
```
