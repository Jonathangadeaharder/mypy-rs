# Wire-churn phase profile and lever-2 falsifier (#63)

Status: measurement only, no production change. Verdict: **NO-GO** under the
pre-registered gate. Companion: issue #63 (closed by this falsifier);
parent attribution `docs/plans/2026-09-19-gap-attribution.md` (#61, merged).

- Head: `origin/main` = `acb2ff40d` (rebased from the `8afb1d4ac` base the
  attribution ran on; the Rust side is identical between the two heads),
  worktree `worktrees/mypy-rs-wire-churn` (branch `perf/wire-decode-churn`).
- Extensions rebuilt from this tree into the standard scratch dirs
  (`/private/tmp/mypy-rs-local-{ast,resolver,typekernel}`, codesigned);
  `scripts/assert_worktree_import.py` exit 0 before every run below.
- Corpus: cold self-check, `mypy_self_check.ini -n0 --no-incremental
  -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`), 378 files,
  "Success: no issues found" on every counted run.
- Evidence: `/private/tmp/wire-churn/` (phase profile runs, bench matrix),
  plus the #61 sample battery reused below
  (`/private/tmp/gap-attrib/round{1,2,3}-on.sample.txt`).

## The pre-registered gate

From issue #63, fixed before any measurement: compute decode+clone+drop+hash
as a share of kernel self. If < 30% of kernel self, the lever is NO-GO:
close with the measured split and no production change. The 30% threshold
was chosen because the lever's stated best case (halving decode+clone) then
recovers < 15% of the +27e9 kernel-self bucket, below the cost of a wire
interning or borrowed-decode rewrite of `wire.rs`.

## What was measured

Two independent routes:

1. **Sample route** (existing #61 battery, same head, 3 rounds): classify
   kernel-self symbols into churn families (`sample_self_split`-style).
   Denominator: kernel self = 6.6% of the on-arm's 430.9e9 instructions
   (3,850/4,234/4,345 samples).
2. **Counter route** (new): mode-gated in-kernel counters
   (`WirePhaseCounters` in `crates/type_kernel/src/wire.rs`, default off,
   one thread-local Cell check per bump) + a per-op instruction microbench
   (`misc/wire_churn_bench.sh`) that converts counts to instructions.

## Sample route: decode+clone+drop+hash = 23.2% of kernel self (median)

| family | round1 | round2 | round3 | median share |
| --- | --- | --- | --- | --- |
| clone(wire::Type, incl. Vec/String clones) | 312 | 357 | 369 | 8.4% |
| drop(wire::Type, drop_glue) | 244 | 246 | 263 | 6.1% |
| hash(sip13 Hasher::write) | 178 | 226 | 241 | 5.3% |
| decode(wire readers) | 152 | 141 | 139 | 3.3% |
| (from_utf8, decode-side but unclassified) | 86 | 83 | 93 | ~2.1% |
| (hashbrown-ops, adjacent) | 107 | 114 | 120 | 2.8% |
| **decode+clone+drop+hash (gate)** | | | | **23.2%** |
| **+ every generous adjacent family** | | | | **28.1%** |

Both readings are under the gate. The 23.2% is the honest four-family
number; the 28.1% is the upper bound if every hashbrown op and every
from_utf8 byte validation is charged to churn as well.

## Counter route: the churn is solve-side, not decode-side

Cold self-check, corrected counters (run A/run B):

| counter | run A | run B |
| --- | --- | --- |
| decode_calls | 1,987,490 | 1,987,403 |
| decode_bytes | 128,099,159 | 128,093,635 |
| decode_nodes | 8,285,797 | 8,285,326 |
| encode_calls | 674,982 | 674,974 |
| encode_bytes | 36,764,505 | 36,764,453 |
| encode_nodes | 2,584,353 | 2,584,337 |
| clone_nodes | 13,825,541 | 13,825,184 |
| hash_bytes (decode-side ExtraAttrs) | 4,679,440 | 4,679,440 |
| hash_ops | 192,676 | 192,676 |
| drop_nodes (creation identity) | 22,111,338 | 22,110,510 |

Two earlier runs with the first counter build (before the inline-read
sites were added to `decode_nodes`) agree on every counter except
`decode_nodes` (7.31M vs 8.29M: the inline reads add ~13%), which is
why the corrected definition is the one reported.

Per-op instruction costs from the microbench (median of 3 runs at 1M
iterations; the `clone_drop`-vs-`clone_forget` drop cross-check agrees
within 1 instruction):

| fixture (nodes) | decode/node | drop/node | clone/node |
| --- | --- | --- | --- |
| any (1) | 248.4 | 49.0 | 76.9 |
| instance (3) | 598.4 | 165.5 | 423.5 |
| callable (5) | 707.0 | 211.5 | 512.9 |

Sample-implied per-node averages sit inside the fixture band (clone ~173,
drop ~78, decode ~185 instructions/node at the corrected counts), which
reconciles the two routes: the bench fixtures are structure-heavier than
the corpus mix, and bench per-op costs include allocator instructions that
the #61 decomposition books under the allocator/GC bucket, not kernel
self. Multiplying the corrected counts by the sample-implied per-node
costs reproduces the sample families to within 5% (clone 13.83M x 173 =
2.39e9; drop 22.11M x 78 = 1.73e9; decode 8.29M x 185 = 1.53e9).

**Caller attribution (the decisive split).** Walking the sample trees to
the topmost non-clone/non-glue ancestor:

- clone frames (3,851 over 3 rounds, incl. nested Vec/String clone
  variants): meet 576, checkmember 540, expandtype 308, freshen 249,
  constraints 249, cond_types 218, subtypes 202, typeops 172, ... —
  **~97% solve-side**; only 132 frames (~3.4%) sit under
  `wire::read/write`.
- drop frames: nearest ancestors are `expandtype::expand_type_inner`,
  `expand_type_tuple_with_fallback`, `remove_trivial_*` — solve-side
  temporaries.
- sip13 hash frames: semanal_shared maps, plugin_hooks registry,
  module_resolver, typeinfo NativeTypeResolver, subtypes/checkmember
  lookups — resolver/solve-side lookups, not ExtraAttrs. The decode-side
  ExtraAttrs hash the counters measure (4.68 MB of keys) is 2-3 orders
  of magnitude below the sample hash traffic in instruction terms.

## Verdict

**NO-GO.** Both routes put decode+clone+drop+hash under 30% of kernel
self (23.2% median, 23.1% when recomputed from the corrected counts;
28.0-28.1% with every adjacent family charged to churn), and the caller
attribution shows the churn the lever could actually touch is a small
fraction of even that: interning decoded trees or borrowed decode
addresses the decode readers (3.3% of kernel self, 5.4% incl. from_utf8)
plus at most ~3% of clone and a sliver of drop and hash, for a total
addressable share of roughly 6%. The clone/drop churn is a property of
the solve algorithms building temporary trees (meet, checkmember,
expandtype, freshen, constraints), not of the wire decode path, and the
hash churn is string-keyed map lookups in the resolver and
semantic-analysis snapshots. Phase 2 (interning / borrowed decode) is
shelved; a solve-side tree-sharing lever, if ever pursued, is a
different issue with its own falsifier.

What ships: the mode-off counters (`WirePhaseCounters`), the probe
(`misc/wire_churn_phase_profile.py`), and the bench driver
(`misc/wire_churn_bench.sh`) — the instrumentation this record rests on,
inert by default.

Measurement caveats, stated rather than hidden:

- `drop_nodes` is creation identity (`decode_nodes + clone_nodes`):
  `impl Drop for Type` would forbid the crate's 63 by-value destructures
  (E0509), a production change beyond a probe. The sample-side drop_glue
  share is the cross-check.
- `decode_nodes` counts every materialized `wire::Type` value: `read_type`
  entries plus the inline reads that bypass it (type-var-like elements,
  `Overloaded` items, the five inline `read_instance` fallbacks).
- The bench's `clone_forget`/`decode_forget` arms leak their trees
  intentionally (`std::mem::forget` isolates drop from the op cost);
  processes exit immediately after.
