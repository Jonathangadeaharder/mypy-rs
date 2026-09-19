# Type-arena amortization Step 0: constructions vs funnel appearances (#81)

Status: measurement only, no production change. Verdict: **NO-GO** under the
pre-registered gate, for every family in the immutable-leaf set: constructions
outnumber serialize-funnel appearances by 17-322x for the families that appear
at all, and two families never appear at the funnel, so every mint for them
would be added cost with no corresponding walk to delete. This record closes
the #72 step 1 Lane 1 falsifier; per the pre-registration no flip lane starts
on this slice shape. Companion: issue #81; design brief
`docs/plans/2026-09-19-type-owner-inversion-brief.md` (section 5 is the
pre-registered gate and the probe spec); architecture
`docs/adrs/0007-rust-owned-type-representation.md`; pricing arms from #70
(`docs/plans/2026-09-19-blob-marshal-falsifier.md:82`, `:110`) and the #71
Step-0 walk prices (`docs/plans/2026-09-19-subtype-residency-step0.md`).

- Head: `origin/main` = `3f81b9f7f`, worktree
  `worktrees/mypy-rs-arena-step0` (branch `perf/arena-amortization-step0`);
  no Rust change, no mypy/ change, one default-off probe driver in misc/.
- Extensions: type_kernel built from this tree into the private scratch
  `/private/tmp/mypy-rs-local-typekernel-arena-step0/` (codesigned); the
  shared `/private/tmp/mypy-rs-local-{ast,resolver}` scratch dirs on
  PYTHONPATH (current for this head: no commit after their build touches
  those crates). `scripts/assert_worktree_import.py` exit 0 before every
  counted run, from inside the worktree.
- Corpus: cold self-check, `mypy_self_check.ini -n0 --no-incremental
  -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`), 378 files,
  "Success: no issues found in 378 source files" on every counted run and on
  the probe-off baseline.
- Evidence: `/private/tmp/arena-step0/` (probe runs 1-2 and the probe-off
  baseline, stdout + /usr/bin/time logs).
- Counters are counts and load-invariant; instructions and wall seconds are
  recorded, never gated, and probe runs are never used as instruction
  baselines (the #71 Step-0 probe-pin discipline).

## The pre-registered gate (from #81 / brief section 5, verbatim in force)

> If `constructions / appearances` for the immutable-leaf family set exceeds
> **1.0** (each object used at most once), the lane reports the slice shape
> **NO-GO for that family and stops**. No flip lane starts.

Pricing, fixed before this lane ran: mint arm at the per-crossing floor
(~139 instr/arg marshal, blob falsifier `:110`, plus the 1-arg crossing floor
105.9, `:82`); walk arm at serialize hit 1,815.3 / miss 15,265.2 instr per
appearance (#71 Step-0 table; `ser_miss_any` is the AnyType miss price).

## What the probe counts

`misc/arena_amortization_step0_probe.py` runs the cold self-check with
runtime-only patches over `mypy.types` (nothing in mypy/ changes; the probe
exists only when this driver runs it):

- **constructions**: `__init__` wrappers on the five family classes. Every
  in-tree creation path routes through `__init__` (constructors, `read`,
  `deserialize`, `copytype`'s copier and its native decode path); there is no
  `copy.copy`/pickle channel on `Type` objects (mypyc compat, #58).
- **appearances at the funnel**: a wrapper on
  `_serialize_type_for_visitor` (`mypy/types.py:4746`) counts top-level
  appearances; a wrapper on `_write_type_cached` counts nested appearances
  only while a funnel call is in flight, so cache I/O
  (`mypy/cache_data.py:55`) and the copy seam (`mypy/copytype.py:72`), which
  reach the same helpers outside the funnel, are excluded by construction.
  A nested hit served from the F3 cache counts as an appearance (the splice
  is per-appearance work the flip deletes); a child inside a top-level cache
  hit is never walked and never counted.
- **serve classes**: per appearance, replicating the wrapped functions'
  decision order exactly (phase gate, then the nested-only librt splice
  availability, then the entry identity check).
- **refusal classes**: unconstructed appearances (an object with no observed
  construction, i.e. pre-arming constants), id recycling among appearing
  objects (unreachable while the probe pins appearing objects, the same
  discipline as the F3 cache itself), funnel re-entrancy mid-walk, and
  family entries with a tvar fingerprint (impossible for a leaf).

The driver prints the report on every exit path and refuses its own evidence
(run failed, zero funnel calls, no constructions or appearances, any
anomaly class nonzero). Both counted runs exited 0 with zero anomalies and
zero unconstructed appearances; funnel calls 66,003 in both runs, kernel
visitor active.

## Probe populations (two counted runs; spread <= 0.001%)

| counter | run 1 | run 2 |
| --- | --- | --- |
| funnel calls (all types) | 66,003 | 66,003 |
| AnyType constructions | 542,668 | 542,665 |
| AnyType first / total appearances | 26,290 / 31,698 | 26,290 / 31,698 |
| AnyType top / nested | 2,442 / 29,256 | 2,442 / 29,256 |
| AnyType serve hit / miss / cache_off | 1,759 / 302 / 29,637 | 1,759 / 302 / 29,637 |
| UninhabitedType constructions | 54,117 | 54,117 |
| UninhabitedType first / total | 60 / 168 | 60 / 168 |
| UninhabitedType top / nested / serve | 0 / 168 / 0-0-168 | 0 / 168 / 0-0-168 |
| NoneType constructions | 67,069 | 67,069 |
| NoneType first / total | 2,776 / 3,346 | 2,776 / 3,346 |
| NoneType top / nested | 215 / 3,131 | 215 / 3,131 |
| NoneType serve hit / miss / cache_off | 81 / 132 / 3,133 | 81 / 132 / 3,133 |
| ErasedType constructions / appearances | 26,515 / 0 | 26,515 / 0 |
| DeletedType constructions / appearances | 169 / 0 | 169 / 0 |
| family set constructions | 690,538 | 690,535 |
| family set first / total appearances | 29,126 / 35,212 | 29,126 / 35,212 |
| refusal classes (all, both runs) | 0 | 0 |

Instructions retired: probe-on 438.21e9 / 438.83e9, probe-off baseline
430.06e9 (same head, same PYTHONPATH), so the probe costs ~1.9-2.0% and its
runs are counter sources only. Peak RSS 683-687 MB probe-on vs 675 MB
probe-off (the probe pins appearing family objects, 29,126 distinct).

## Gate arithmetic

Per family, constructions / total appearances:

| family | constructions | first | total appearances | ratio | verdict |
| --- | --- | --- | --- | --- | --- |
| AnyType | 542,668 | 26,290 | 31,698 | **17.12** | NO-GO |
| UninhabitedType | 54,117 | 60 | 168 | **322.13** | NO-GO |
| NoneType | 67,069 | 2,776 | 3,346 | **20.05** | NO-GO |
| ErasedType | 26,515 | 0 | 0 | no appearances | NO-GO |
| DeletedType | 169 | 0 | 0 | no appearances | NO-GO |
| family set | 690,538 | 29,126 | 35,212 | **19.61** | NO-GO |

The gate line is 1.0; every family clears it by more than an order of
magnitude, and ErasedType / DeletedType never reach the funnel at all (their
ratio is unbounded: constructions with zero appearances). The named break
point is the population itself: 661,412 of 690,538 constructions (95.78%)
produce an object that is never serialized at the funnel on this build, so
the mint arm pays ~19.6 crossings for every walk it could delete.

**Robust to the second funnel.** The subtype seam has its own serializer,
`_serialize_type` (`mypy/subtypes.py:297`), which this probe does not count
(the pre-registration names `_serialize_type_for_visitor`,
`mypy/types.py:4746`). #71 measured that seam's operand traffic at 299,558
appearances per cold self-check, all types combined. Even if every one of
those appearances were an immutable leaf and all were additional to the
35,212 counted here, the set ratio would be 690,538 / 334,770 = 2.06 > 1.0.
The gate fires under the most generous reading of the unmeasured funnel.

## Priced context (reported, not gating; the gate is the falsifier)

Applying the brief's own arms to the measured populations, with the serve
split above and the cache_off class priced as a full walk (that is what it
is: no cache read, no cache store):

- Mint floor per construction (1-arg crossing 105.9 + ~139 per marshaled
  data argument): AnyType ~522.9 (3 data args), UninhabitedType and
  DeletedType ~244.9 (1), NoneType and ErasedType 105.9 (0). Weighted:
  **0.307e9** instructions per build, a floor.
- Walk arm per appearance (#71 prices; `ser_miss_any` 15,265.2 is the only
  leaf-priced miss figure, AnyType; NoneType / UninhabitedType walks are
  2-byte singleton tags and cheaper, so their miss price is an upper
  bound): all-hit floor 0.064e9; as-measured classes
  1,840 x 1,815.3 + 33,372 x 15,265.2 = **0.513e9**; all-miss ceiling
  0.538e9.

The as-measured band straddles the mint floor, and that is reported rather
than hidden: for AnyType alone the deleted walks could price above the
mint floor. Three things keep this from reopening the lane. First, the
pre-registered gate governs; it fires at 17-322x, and re-deriving a
friendlier gate after seeing the numbers is exactly the move the
pre-registration forbids. Second, the upside is bounded at
~0.51e9 - 0.31e9 = 0.2e9 on a 430e9 build (~0.05%), an order of magnitude
below any milestone the #72 rule would trade a representation inversion
for, while the risk side (facade contract, plugin surface, slot-layout
break) is unchanged. Third, the population structure is the failure mode
the gate encodes: 95.78% of mints have no corresponding appearance, so the
flip adds a crossing on every construction site (a hot path) to delete
work on a path most constructed objects never reach. Whether a later lane
pre-registers a gate on the priced delta rather than the count ratio is a
new decision for #72, made before measurement, not a reinterpretation of
this one.

## What ships

- `misc/arena_amortization_step0_probe.py`, the default-off probe driver:
  runtime-only patches over `mypy.types`, no mypy/ or crates/ change, a
  smoke-args hook for small-corpus validation, report on every exit path,
  and evidence-refusal guards on the pre-registered conditions.
- This record.

## Caveats, stated rather than hidden

- The appearance base is the funnel the pre-registration names. The
  subtype seam serializer (`mypy/subtypes.py:297`) is a second,
  unmeasured funnel; the robustness bound above prices the whole #71
  population into it and the gate still fires.
- Nested serve classes reflect the shipped venv configuration: the PyPI
  librt wheel does not export `write_raw_bytes`
  (`mypy/types.py:29-35`), so nested splices cannot hit and every nested
  appearance in the cache-on phase still walks. Top-level funnel hits
  (1,840 across the set) do not need librt and are measured as hits.
  A librt-scratch configuration would move some nested cache_off events
  into hits, lowering the walk-arm band; the count gate is unaffected.
- The cache_off split is also phase-structural, not only librt: the wire
  cache is disabled outside the checker phase
  (`mypy/semanal_main.py:92`, `mypy/build.py:4808`), so semanal-phase
  funnel appearances walk regardless of librt.
- The #71 walk prices were measured on the subtype-seam serializer with
  Instance-dominated operands; only `ser_miss_any` is a leaf price. The
  as-measured savings figure prices NoneType / UninhabitedType misses at
  the AnyType miss figure and is therefore an upper bound.
- The probe pins the 29,126 distinct appearing family objects (identity
  distinctness, same discipline as the F3 cache) and costs 1.9-2.0%
  instructions; counted runs are counter sources, never baselines. The
  probe-off baseline for the same head and PYTHONPATH is 430.06e9.
- Constructions are counted from probe arming (after `mypy.types` import,
  before `mypy.main.main`); in-tree module import builds no family
  objects, and the unconstructed-appearance refusal class measured zero,
  so the arming boundary is empty on this corpus.
- `--dump-build-stats` output goes to the run stdout in the evidence dir;
  wall clocks are not recorded as evidence (load varied, counts do not).

## Reproduction

```bash
# extensions: build type_kernel into the private scratch (AGENTS.md order)
cargo rustc -p mypy-type-kernel --features extension-module --lib \
  --crate-type cdylib --release -- -C link-arg=-undefined -C link-arg=dynamic_lookup
cp target/release/libtype_kernel.dylib \
  /private/tmp/mypy-rs-local-typekernel-arena-step0/type_kernel.cpython-313-darwin.so
codesign -f -s - /private/tmp/mypy-rs-local-typekernel-arena-step0/*.so

export PYTHONPATH=/private/tmp/mypy-rs-local-typekernel-arena-step0:\
/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver
# from the worktree root, before every counted run:
.venv/bin/python scripts/assert_worktree_import.py .

# counted probe runs (pool corpus-class)
MYPY_NUM_WORKERS=0 /usr/bin/time -l \
  .venv/bin/python misc/arena_amortization_step0_probe.py

# probe-off baseline
MYPY_NUM_WORKERS=0 /usr/bin/time -l .venv/bin/python -m mypy \
  --config-file mypy_self_check.ini -n0 --no-incremental -p mypy -p mypyc
```
