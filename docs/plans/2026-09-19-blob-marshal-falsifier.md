# Blob-marshal coded-seam falsifier and arg-packing ceiling (#70)

Status: measurement only, no production change. Verdict: **NO-GO** under the
pre-registered gate; arg packing is dropped, and call batching is measured
structurally blocked, so the boundary levers on coded seams are exhausted.
Companion: issue #70 (closed by this falsifier); parent attribution
`docs/plans/2026-09-19-gap-attribution.md` (#61, lever 5); batch history
#812/#33.

- Head: `origin/main` = `d4e898798` (2026-09-19), worktree
  `worktrees/mypy-rs-blob-marshal` (branch `perf/blob-marshal-falsifier`).
- The type_kernel extension was rebuilt from this tree into
  `/private/tmp/mypy-rs-local-typekernel` (codesigned; a first build from
  the main checkout was caught by the compile log and redone from the
  worktree). `scripts/assert_worktree_import.py` exit 0 from inside the
  worktree before the bench and the audit.
- Corpus for the projection leg: cold self-check, `mypy_self_check.ini
  -n0 --no-incremental -p mypy -p mypyc`, single process, 378 files,
  "Success: no issues found" (`/private/tmp/blob-marshal/audit.txt`).
- Load: the bench ran at load1 ~44 (sibling lanes, plus the audit corpus
  running concurrently for two of the three reps). Instructions are
  load-invariant (#61 verified agreement to 0.5% at load1 35-52); wall
  seconds are recorded but not used for any conclusion.
- Evidence: `/private/tmp/blob-marshal/` (`microbench.txt` with all 3 reps
  per bench, `audit.txt` wire-traffic report, `arg_counts.tsv` per-seam
  PyO3 argument counts extracted from `crates/type_kernel/src/`).

## The pre-registered gate

From issue #70, fixed before any measurement: bench a prebuilt
single-buffer (blob) variant of a hot 12-arg coded seam
(`rust_is_subtype_coded`) against the current 12-arg form,
instruction-denominated, engagement-asserted, same-shape control loop.
**GO for arg-packing only if the blob form cuts the coded-call cost by
>=70%** (toward the 1-arg floor plus the one buffer build) **and the
projected global saving across the top coded seams is >=1e9
instructions.** Otherwise NO-GO: record the measured ceiling and drop arg
packing. The gate decides pursue vs drop, not parity.

## What was built

- `rust_is_subtype_blob_bench` (`crates/type_kernel/src/subtypes.rs`):
  a bench-only `#[pyfunction]`, registered in the module surface but with
  no production caller (`rg rust_is_subtype_blob_bench mypy/` = 0 hits;
  the audit run confirms it in the zero-calls section). It takes one
  prebuilt buffer (u16 LE flag mask, u32 LE left length, left bytes,
  right bytes) plus the resolver pointer, decodes to exactly the values
  `rust_is_subtype_coded` receives, and mirrors its body 1:1, codes
  included. Inert by construction; no production seam changed.
- `misc/blob_marshal_bench.py`: the measurement driver, in the
  `misc/gap_attrib_microbench.py` lineage. Every bench runs its operation
  against a control loop of identical shape (same-arity no-op), under
  `/usr/bin/time -l`, 3 reps, medians reported with min/max spread. Every
  measured subprocess asserts engagement BEFORE the loop: the blob entry
  must agree with the 12-arg coded entry on a true pair (code 1) and a
  false pair (code 0), a truncated blob must defer (-1), the blob layout
  must round-trip, and the control bench's counter must move.

## Arms

- `coded12`: the current 12-arg `rust_is_subtype_coded` call, same fixture
  and argument shape as #61's `pyo3_13args` bench (identity Instance pair,
  all context flags False, mask 0).
- `blob_call`: `rust_is_subtype_blob_bench` with the buffer prebuilt
  outside the loop (the Python-side buffer build excluded).
- `blob_build_call`: the blob call including one per-call Python-side
  buffer build (`struct.pack("<HI") + lb + rb`, the exact pack a
  production shim would do on top of the wire-cache bytes it already
  holds). This is the deployable form.
- `blob_build`: the Python-side buffer build alone.
- `pyo3_1arg`: the bare 1-arg crossing (counter record), the control
  floor.

## Results (instructions per call, median of 3 reps, 1M/2M iters)

| bench | median | min | max |
| --- | --- | --- | --- |
| coded12 (current 12-arg) | 4,522.4 | 4,516.5 | 4,522.9 |
| blob_call (build excluded) | 2,989.8 | 2,949.5 | 3,265.4 |
| blob_build_call (incl. build) | 5,787.2 | 5,773.8 | 5,797.2 |
| blob_build alone | 1,547.8 | 1,525.3 | 1,550.6 |
| pyo3_1arg control | 105.9 | 103.7 | 108.7 |

- Blob cut of the coded-call cost, build excluded: **33.9%**
  (4,522.4 -> 2,989.8). Gate needs >=70%. **FAIL.**
- Blob cut including the amortized buffer build: **-27.9%**
  (4,522.4 -> 5,787.2): the deployable form is NET NEGATIVE. The Python
  buffer build (1,547.8 instr) costs slightly more than the entire
  marshaling it replaces (1,532.6 instr).
- Ranges do not overlap between arms; the sign and order are stable
  across all three reps. The `blob_call` spread (10.6%) is the widest and
  coincides with the concurrent audit corpus; the conclusion does not
  depend on it (min-blob 2,949.5 vs max-coded 4,522.9).
- The coded12 figure reproduces #61's 4,410.9 to +2.5% (different head).
- Post-lint reproduction (rebuilt extension, final file): two full re-runs
  agree with the medians above to 0.2-0.6% on every bench (coded12
  4,521.2, blob_call 2,973.2, blob_build_call 5,779.1, blob_build 1,545.5,
  pyo3_1arg 107.6); the table's medians are the original run's, and
  `microbench_final.txt` holds the final clean pass.

## Decomposition: #61's 392 instr/arg was an over-attribution

The blob arms separate what #61's two benches could not (its 1-arg
control was a different function with a trivial body):

- The coded body itself (decode 2 one-node Instance trees, expand aliases,
  solve, code out) costs ~2,890 instructions (blob_call minus its 2-arg
  crossing).
- The 12-arg marshaling above the 2-arg blob floor is 1,532.6
  instructions across 11 absorbed arguments: **~139 instr/arg, not ~392**.
  The 392 figure charged the body work to marshaling.
- Consequence for the gate's framing: even a perfect arg-packing win that
  reached the 1-arg floor could remove at most ~34% of the coded-call
  cost on this fixture, under the 70% threshold before any buffer-build
  cost is paid. Real trees average ~4.6 nodes (#61), making the body share
  larger and the marshal share smaller, so 34% is an upper bound.

## Projected global ceiling across coded seams

Per-seam calls from the fresh audit run (total 2,428,255 seam calls, 305
called seams); per-seam PyO3 argument counts extracted from
`crates/type_kernel/src/` (u16-generic signatures included; all 305
called seams resolved). Model: saving per call = (N-2) x per-arg rate
(every coded argument past a kept blob+pointer pair packs into the
buffer), summed over the 158 called seams with N >= 3 (1,469,830 calls).
This is a ceiling: it ignores per-call buffer builds (measured net
negative above) and any argument that is a live Python object rather
than a scalar/blob.

- At the measured rate (139.3 instr/arg): **0.98e9 instructions** — under
  the 1e9 gate leg even as an unrealizable ceiling.
- At #61's refuted optimistic rate (392 instr/arg): 2.77e9 — reported only
  to show the gate leg was not the decisive failure.

Top contributors at the measured rate (M instr):

| seam | calls | PyO3 args | ceiling (M) |
| --- | --- | --- | --- |
| rust_classify_special_unbound | 126,011 | 20 | 316.0 |
| rust_analyze_instance_member_dispatch | 111,101 | 9 | 108.4 |
| rust_classify_check_final | 57,084 | 6 | 31.8 |
| rust_is_subtype_coded | 22,590 | 12 | 31.5 |
| rust_dangerous_comparison | 17,848 | 14 | 29.8 |
| rust_classify_missing_annotations | 18,070 | 13 | 27.7 |
| rust_make_simplified_union | 30,764 | 8 | 25.7 |
| rust_classify_unbound_front | 10,645 | 19 | 25.2 |
| rust_classify_return_stmt_post | 16,083 | 12 | 22.4 |
| rust_infer_constraints_full | 24,156 | 8 | 20.2 |

## The structural batchability question

Issue #70's second question: can an outer checker operation submit
several subtype/member/expansion questions in one packet sharing one
serialized environment, or are per-op results structurally required?
Answer from the call sites: **per-op results are structurally required;
batching is blocked at the call-path level.**

- The native coded answer is consumed synchronously at the seam:
  `mypy/subtypes.py:958-994` — `code = coded_entry(...)` then
  `return code != 0`; a -1 defers THIS pair to the Python visitor
  (`subtypes.py:995`).
- The question stream is data-dependent, not pre-submittable:
  `SubtypeVisitor.visit_instance` asks promotion questions inside a
  short-circuiting `any(...)` (`mypy/subtypes.py:1186-1188`), so each
  answer determines whether later questions are asked at all; the same
  shape holds for `all(is_subtype(...))` over TypeVar values
  (`mypy/subtypes.py:892-897`) and the variance dispatch in
  `check_type_parameter` (`mypy/subtypes.py:1004-1018`), where the
  answer to the classify call selects WHICH subtype question to ask.
- Outer checker operations branch on each answer immediately:
  `mypy/checker.py:1499` (sequence check gates a diagnostic),
  `mypy/checker.py:2069-2072` (overload return comparison, `or`
  short-circuit), `mypy/checkexpr.py:3964` (per-argument compatibility
  inside the argmap loop), `mypy/checkmember.py:1877-1880`.
- Direct experimental evidence: the one batching experiment this fork has
  run (#812, removed in #33) buffered pairs but could not deliver
  answers from the buffer. The removed code's own comment
  (commit `95412b574`, `mypy/subtypes.py`): "Not flushed yet: the
  current pair's answer is not known; fall through to the single-pair
  path for THIS call so the caller still gets a bool." The batch could
  only prefetch answers the Python visitor computed anyway;
  #33 (`fc675298d`) removed it as net-negative: 27,360 scalar calls
  across 11,572 flushes, ~189 ms of redundant Rust re-evaluation per
  cold self-check.
- Therefore a batch that shares one serialized environment requires the
  outer checker operation itself to move into Rust — an ownership flip
  (the Phases F-H strangler direction), not a call-path change.

## Verdict

**NO-GO.** The blob form cuts the coded call by 33.9% with the buffer
build hidden, and is net-negative (-27.9%) in the deployable
build-included form, against a >=70% gate; the honest global ceiling is
0.98e9 instructions against a >=1e9 gate, and that ceiling is already
refuted by the measured build cost. Arg packing is dropped. With batching
structurally blocked at the call-path level, there is no remaining
boundary lever on coded-seam crossing costs: the #61 conclusion stands
with sharper numbers — parity requires ownership moves (results, decoded
trees, the snapshot held across calls in the kernel), not a cheaper call
path.

What ships: the inert blob entry point
(`rust_is_subtype_blob_bench`), the bench driver
(`misc/blob_marshal_bench.py`), and this record. No production seam
change.

## Measurement caveats, stated rather than hidden

- The bench fixtures are 1-node Instance pairs. Real corpus trees are
  ~4.6 nodes on average, which grows the body share and shrinks the
  marshal share; the 33.9% cut is therefore an upper bound on the real
  cut, strengthening the NO-GO.
- The blob build was measured on ~30-byte wire blobs; the build cost
  scales with blob size, so for real trees the net-negative margin only
  grows.
- `blob_call`'s rep spread (2,949.5-3,265.4) overlaps the concurrent
  audit corpus; all other benches spread <2.6%. No arm's range touches
  another's, and #61's control fixture (`pyo3_1arg` at 105.9 vs 102.1)
  reproduces within 3.7%.
- The projection charges every non-kept PyO3 argument the measured
  scalar rate; live-object arguments (Type lists, resolver handles) would
  need their own serialization inside the blob, costing more, not less.
- The audit's per-seam call counts are exact in-process counters
  (MYPY_SERIALIZE_STATS=1); the argument counts are static, from the
  `#[pyo3(signature)]` attributes on this head.

## Gates run

- `scripts/assert_worktree_import.py .` from inside the worktree: exit 0
  before the bench and the audit.
- `cargo test -p mypy-type-kernel` (sem pool, build-class): 2,914 passed,
  0 failed (includes 3 new unit tests for the blob parse and flag
  unpacking).
- The audit run completed "Success: no issues found in 378 source files".
- All bench engagement asserts pass, re-run clean after the final lint
  pass (`uvx black --check`, `uvx ruff check` on
  `misc/blob_marshal_bench.py`; `cargo fmt` + `cargo clippy` clean on the
  touched crate).
- Diff: one inert `#[pyfunction]` + parse helpers + tests in
  `crates/type_kernel/src/subtypes.rs`, the bench script, this record.

## Reproduction

From the worktree root, extensions built to the standard scratch dirs
(see `docs/native-build-reference.md`), through the sem pool
(`/private/tmp/mypy-rs-sem.sh run 1` for the build-class bench and
build, `run 2` for the corpus-class audit):

```bash
/private/tmp/mypy-rs-sem.sh run 1 cargo rustc -p mypy-type-kernel \
  --features extension-module --lib --crate-type cdylib --release \
  -- -C link-arg=-undefined -C link-arg=dynamic_lookup
cp target/release/libtype_kernel.dylib \
  /private/tmp/mypy-rs-local-typekernel/type_kernel.cpython-313-darwin.so
codesign -f -s - /private/tmp/mypy-rs-local-typekernel/*.so
.venv/bin/python scripts/assert_worktree_import.py .
/private/tmp/mypy-rs-sem.sh run 1 .venv/bin/python misc/blob_marshal_bench.py

/private/tmp/mypy-rs-sem.sh run 2 env MYPY_AUDIT_ROOT=$PWD \
  MYPY_SERIALIZE_STATS=1 \
  MYPY_AUDIT_ARGS="-n0 --no-incremental --dump-build-stats -p mypy -p mypyc" \
  PYTHONPATH=/private/tmp/mypy-rs-local-typekernel:/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver \
  .venv/bin/python misc/audit_wire_traffic.py
```

The projection joins the audit's per-seam call counts with the PyO3
argument counts; the extraction snippet and join live in the lane notes
(`/private/tmp/blob-marshal/arg_counts.tsv`) and are fully determined by
the two committed artifacts (audit report format, crate source).
