# Kernel-on vs kernel-off instruction gap attribution (#61)

Status: measurement only, no production change. Counts the native type
kernel's instruction cost on this head, decomposes it into buckets, ranks
the levers. Companion: issue #61; upstream #1878 (parity target);
#58 (identity-memo NO-GO); #1861 (crossing costs).

- Head: `origin/main` = `8afb1d4ac` (2026-09-19), worktree
  `worktrees/mypy-rs-gap-attrib` (branch `perf/gap-attribution`).
- Extensions rebuilt from this tree into the standard scratch dirs
  (`/private/tmp/mypy-rs-local-{ast,resolver,typekernel}`, codesigned);
  `scripts/assert_worktree_import.py` exit 0 from inside the worktree
  before every family below.
- Corpus: cold self-check, `mypy_self_check.ini -n0 --no-incremental
  -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`, which beats
  the ini's `num_workers = 4`), 378 files, "Success: no issues found".
  Arms: kernel-on (production default) vs `--no-native-type-kernel`.
  The native parser and native resolver are ON in both arms (production
  defaults), so the delta isolates the type-kernel seam.
- Load: the battery ran at load1 35-52 (sibling Lean lane). Instructions
  and cycles are load-invariant; wall seconds are recorded but flagged
  load-contaminated and are not used for any conclusion.
- Evidence lives under `/private/tmp/gap-attrib/` (battery
  `round{1,2,3}-{on,off}.{log,time,out,sample.txt}`, `audit.txt`,
  `wire_prep.txt`, `microbench.txt`, `sample_split.txt`,
  `profile_diff.txt`, `decomposition.txt`).

## The gap

`/usr/bin/time -l` "instructions retired", 3 interleaved paired rounds
(`misc/gap_attrib_rounds.sh`):

| round | kernel-on | kernel-off | paired delta |
| --- | --- | --- | --- |
| 1 | 430,921,724,459 | 348,833,373,380 | +82.088e9 |
| 2 | 431,179,182,912 | 348,803,116,942 | +82.376e9 |
| 3 | 430,454,502,935 | 347,976,687,322 | +82.478e9 |

**Median paired gap +82.38e9 instructions (+23.6%)**, spread 0.39e9
(0.5%). Cycles 199.5e9 vs 144.7e9: IPC drops 2.41 -> 2.16, marginal IPC
of the added work **1.50** (memory-bound, not compute-bound). Peak RSS
706MB vs 585MB (**+116 MiB**). Wall 68-74s vs 51-55s (load-contaminated).

## Instruments, and what each one is trusted for

- `/usr/bin/time -l` instructions: the primary currency. Load-invariant
  (verified: three rounds at load1 35-52 agree to 0.5%).
- `sample(1)` (`misc/gap_attrib_sample_split.py`): wall-share by module.
  Load inflates `__psynch_cvwait` (8.9-13.7% both arms) but not the
  shares of compute modules; kernel self-share is the clean signal.
- cProfile (`misc/gap_attrib_cprofile.sh`): hook cost measured directly
  by re-running both arms under `/usr/bin/time -l`: profiled runs retire
  1,228.8e9 (on) / 1,089.5e9 (off) vs 430.9e9 / 348.8e9 unprofiled, a
  blended **1,661 / 1,667 instr per profiled call** (+797.9e9 / +740.7e9
  inflation, two independent runs of the same instrument agreeing). The
  serialize bucket's cProfile tottime growth is 118% explained by its
  49.6M call-count growth alone. Consequence, used throughout:
  **cProfile ncalls are exact and reproduce across runs to <0.01%**
  (serialize dnc 21,199,139 vs 21,198,506 in two runs); cProfile
  tottime deltas are hook-dominated and are NOT used for magnitudes.
- Wire-prep ledger (`MYPY_WIRE_PREP=1` patch, `misc/` probe lineage of
  `measure_wire_prep.py`): `perf_counter` clocks around 845 patched
  seams, 46 serializers, 24 adapters. Conservation identity PASS.
- `MYPY_SERIALIZE_STATS=1 CLOCK=1` audit (`misc/audit_wire_traffic.py`):
  in-process counters, stderr report archived as `audit.txt`.
- Microbench (`misc/gap_attrib_microbench.py`): instruction-denominated
  per-op costs, each against a same-shape control loop, with engagement
  asserts in every measured subprocess.

Counter denominators (do not mix):

- serialize **calls** 839,781 = top-level `_write_type_cached`-driven
  entries (audit). Wire-prep **entries** 1,756,598 = every patched
  serializer invocation including nested ones (~2.1 per call; ~4.6
  entries per non-hit top-level build).
- `_subtype_answers` lookups 149,685 / dedup hits 127,098 (85%).
- Total Rust seam calls 2,427,595 across 305 called seams (830
  registered, armed gates only; a lower bound on the registered
  surface); 4,586 defers (0.19%).

## Decomposition

Magnitudes come from load-independent sources (sample shares x arm
instructions; ledger clocks x throughput; microbench x exact counts).
cProfile supplies structure and counts only.

| bucket | instr (band) | share | basis |
| --- | --- | --- | --- |
| Rust kernel self (wire decode, solve, re-encode) | +27e9 (25-29) | 33% | sample: kernel self 6.6% on vs 0.4% off, x arm instructions |
| Python serialize prep (wire bytes: per-seam args, dedup keys, snapshot bytes, wire-cache upkeep, unconsumed) | +20e9 (16-24) | 24% | ledger 3.45s on-arm (off baseline ~12% from cProfile ratios); microbench mix model agrees (1.76M entries x 0.15x1.8k + 0.85x15.3k) |
| Deser/rebuild back to Python objects (89k rebuilds + 206k fixup walks + 468k copy.copy) | +5e9 (3-9) | 6% | exact counts x modeled per-object cost |
| Seam crossings + gates glue (2.43M calls, arg marshaling 107-4,412 instr/call) | +5e9 (3-8) | 6% | microbench x counts |
| Allocator/GC amplification (malloc self 0.5% -> 2.2%, memset 3.1% -> 4.2%, +116 MiB working set) | +7e9 (4-12) | 9% | sample share deltas x arm instructions |
| **Subtotal identified** | **+64e9 (51-82)** | **78%** | |
| Residual (per-unit model error: benched 1-2 node fixtures vs real ~4.6-node trees; denominator spread; GC passes; second-order dict/attr growth) | +18e9 (0-31) | 22% | gap - subtotal |

Two independent estimates of the serialize-prep bucket agree within
~15% (ledger-clock conversion ~20.6e9 vs microbench-mix ~23.3e9,
less the ~12% off-arm baseline). The residual is an honest
error term, not a hidden bucket: the point estimates leave ~22%
unaccounted, and the combined bands close the gap.

Sample split (median of 3 rounds per arm, `sample_split.txt`):
python3.13 74.7% vs 81.6%; type_kernel Rust self **6.6% vs 0.4%**;
libsystem_malloc 2.2% vs 0.5%; platform memset 4.2% vs 3.1%;
ast_serialize ~325 absolute samples in BOTH arms (parser identical,
sanity check). Top kernel self symbols every round:
`wire::Type` `Clone::clone` (215-259), `drop_glue` (212-241),
sip13 `Hasher::write` (226-257): wire-structure churn, not solving,
is what the Rust side visibly spends its self time on.

## Wire-prep ledger (Python side, on-arm)

- Seam-attributed prep 3.529s (useful 3.196s / deferred 0.007s /
  raised 0.002s) + unconsumed 0.244s = 3.773s of serialize entries;
  adapter windows 0.324s; adapter residual **1.423s** = window time no
  ledger sees (raw `t.write` buffer prep, gate checks, argument glue).
- **Prep on deferred calls: 0.007s total.** The pre-gate lever (skip
  prep before a call that will defer) is dead on this head: the defer
  rate is 0.19% and its prep share is proportional.
- Top prep site: `subtypes._is_subtype` dedup keys, 0.283s + 0.185s
  consumed (148,874 + 148,873 entries) and 0.243s of the 0.244s total
  unconsumed. 85% of `_subtype_answers` lookups are repeat hits; the
  key bytes are rebuilt for every one.
- 148 of 309 called seams serialize zero prep (answer-only seams).
- Per-seam standouts: `rust_solve_generic_call` 19.8 us prep/call
  (10,054 calls, 12 coded args), `rust_classify_simple_assignment`
  6.4 us/call, `rust_analyze_instance_member_dispatch` 1.8 us/call
  (111,065 calls).

## Microbench (instr per call, engagement-asserted)

| bench | instr/call |
| --- | --- |
| gate_check | 1,628.6 |
| serialize hit (wire cache) | 1,807.8 |
| serialize miss (AnyType, 1 node) | 15,329.1 |
| serialize miss (Instance, 1 node) | 16,116.4 |
| visitor-funnel hit / miss | 1,975.3 / 11,357.5 |
| wire cache store | 775.0 |
| content probe / store | 325.7 / 762.2 |
| PyO3 no-arg (counter read, incl. 9-tuple build) | 775.2 |
| PyO3 1-arg (coded record) | 102.1 |
| PyO3 12-arg coded call | 4,410.9 |
| Rust->Python callback vs direct call | 3,960.5 vs 65.1 |
| e2e is_subtype repeat, native on / off | 28,009.2 / 30,294.7 |
| e2e is_subtype distinct, native on / off | 86,258.1 / 82,162.2 |

Run-to-run spread on repeated benches is 0.3-2%; the e2e distinct
spread is the largest (a second run put the native miss-path penalty at
+4,096 vs +5,436 in the first) - the sign and order of magnitude are
stable.

## H1 answer (crossing costs on this host)

- A bare 1-arg PyO3 crossing is 102 instructions (matches #1861's
  ~48ns/crossing figure). A 12-arg coded call is 4,411 instructions:
  **argument marshaling at ~392 instr/arg is ~40x the crossing itself.**
- Rust->Python callback: 3,960 instructions vs 65 for the direct Python
  call (~60x). A Rust driver issuing Python callbacks per node pays
  more per callback than the Python body it replaces. "Move the caller
  into Rust" pays only if the per-callback body moves with it; this
  confirms #1861's parking of slices 2-3 with instruction-denominated
  numbers.
- The native answer cache is already net-positive on its hit path:
  the e2e repeat arm (cache hit) is CHEAPER native-on (28,009.2 vs
  30,294.7); the distinct arm (serialize + miss) pays ~+4,100-5,400
  across runs. The gap is not the answer cache; it is everything
  around it.

## Lever ranking (ceiling on this head, with falsifier)

1. **Incremental resolver snapshot** - 546 collects re-serialize the
   full graph (`resolver_build_s` 0.808s, 68,154 infos, 56,138
   re-pushed builtins; cache.py `write_str` 239k -> 1.81M calls).
   Delta-serialize or move the snapshot into the resolver.
   Ceiling ~3-5e9 (4-6%). Falsifier: count snapshot entries unchanged
   between consecutive collects; if <50% are unchanged, drop.
2. **Kernel wire-structure churn** - kernel self is 33% of the gap and
   its top self symbols are clone/drop/sip-hash on `wire::Type`.
   Interning decoded trees per bytes blob, or borrowed (non-owning)
   decode, halves decode+clone in the best case.
   Ceiling ~6-13e9 (8-16%). Falsifier: an in-kernel phase profile
   (decode vs solve vs encode counters); if decode+clone < 30% of
   kernel self, drop.
3. **Deser-back avoidance** - 89k full rebuilds (16,965 checkmember
   misses + 64,391 checker decodes + ~8k maptype) plus 206k fixup
   walks and 468k `copy.copy`. Cache rebuilt objects/results keyed on
   the input identity the kernel already holds.
   Ceiling ~2-4e9 (2-5%). Falsifier: count rebuilds whose input bytes
   repeat within a build; if <30% repeat, drop.
4. **`_is_subtype` dedup-key serialization** - 0.47s consumed + 0.24s
   unconsumed per build, 85% repeat lookups. #58 measured the identity
   memo (32.8% memoable) and ended NO-GO: weak identity is impossible
   on slots-based Type, raw-id is unsound (84 recycled ids in one
   build). The only sound paths left are the #58 follow-ups: a
   `__weakref__`-slots slice with its own RSS+instr A/B, or serving
   the answer cache Rust-side keyed on the bytes it already receives
   (removes the Python map, not the key builds).
   Ceiling ~2-4e9 (2-5%), feasibility constrained by #58.
5. **Arg-marshal reduction on coded seams** - `rust_solve_generic_call`
   prep is 19.8 us/call at 12 coded args. Bundling tree args into one
   prebuilt blob + pointer args cuts marshal to the 1-arg floor.
   Ceiling ~1-2e9 (1-2%). Falsifier: bench a blob-variant of
   `rust_is_subtype_coded`; if no better than 12-arg, drop.
6. **Pre-gating deferred calls** - DEAD. 0.007s of prep sits on
   deferred calls; nothing to reclaim.
7. **Allocator/GC amplification** - +116 MiB and doubled malloc/free
   shares shrink automatically with 1-3; no standalone lever.

Realistic combined savings: **12-24e9 (15-29%)**. #1878's parity target
(kernel-on <= Python-only) needs ~82e9 on this head. Per-call
optimization cannot close it; parity requires ownership moves - the
kernel holding results, decoded trees, and the snapshot across calls -
which is the Phases F-H strangler direction, not a cheaper call path.

## Verdict on #1878's framing

- Agree on direction and rough scale: per-call overhead dominates
  (their +33% on their head vs +23.6% here), and the two named buckets
  exist and are real.
- The two-bucket guess is incomplete. Measured buckets are six, and
  the biggest two are NOT the ones named: "Rust decode/encode
  round-trip" is the kernel-self bucket (33%, dominated by
  wire-structure churn, not solving), and "invisible Python wire prep"
  splits into serialize prep (24%), deser-back rebuild + fixup (6%),
  snapshot upkeep (inside the 24%), and adapter glue (6%). Allocator/GC
  amplification (9%) is invisible to both defer A/B and the seam
  ledger.
- Their implied pre-gate lever is dead here (0.007s).
- Their summed per-seam losses (~10% of the gap on their head) are
  consistent with our crossings+glue bucket (~6%) at this corpus size.

## Gates run

- `scripts/assert_worktree_import.py` from inside the worktree: exit 0
  before every family (battery, audit, wire-prep, cProfile, microbench).
- Every measurement run completed "Success: no issues found in 378
  source files" (on-arm, off-arm, profiled arms, audit, wire-prep).
- Microbench: 18 benches, all engagement asserts pass (re-run clean
  after the final lint pass).
- `uvx black --check` + `uvx ruff check` on touched `.py` files,
  `bash -n` on touched `.sh`: clean.
- No production file touched: the diff is five `misc/gap_attrib_*`
  measurement scripts plus this record.

## Reproduction

From the worktree root, extensions built to the standard scratch dirs
(see `docs/native-build-reference.md`), through the sem pool
(`/private/tmp/mypy-rs-sem.sh run 2` for corpus-class, `run 1` for
build-class):

```bash
misc/gap_attrib_rounds.sh                      # paired battery + sample
misc/gap_attrib_cprofile.sh                    # profiled arms, /usr/bin/time -l
misc/gap_attrib_profile_diff.py                 # per-function delta report
misc/gap_attrib_sample_split.py round*.sample.txt
env PYTHONPATH=<scratch dirs> .venv/bin/python misc/gap_attrib_microbench.py
```
