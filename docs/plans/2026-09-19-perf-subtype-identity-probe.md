# Subtype identity-repeat probe and front-memo gate (#58)

Status: measurement + NO-GO disposition. Counts only; no production
change shipped. Companion: issue #58, upstream #1878 / #1624.

- Head: `origin/main` = `5a2c30466` (2026-09-19), worktree
  `worktrees/mypy-rs-subtype-memo` (branch `perf/subtype-identity-memo`).
- Extensions: rebuilt from this tree into the standard scratch dirs
  (`/private/tmp/mypy-rs-local-{ast,resolver,typekernel}`); the shared
  type_kernel `.so` was stale (pre-#56 typeview prototype) and was
  rebuilt before any measurement.
- Corpus: cold self-check, `mypy_self_check.ini -n0 --no-incremental
  -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`, which beats
  the ini's `num_workers = 4`; the same single-process shape the wire
  audit uses, required for in-process counters), 378 files,
  "Success: no issues found". `--dump-build-stats` added as the probe's
  reporting channel (same flag the audit baseline used).

## Pre-registered gate

Fixed before measuring, from the issue text: adopt a front memo iff the
memoable identity-repeat rate is at least 30% of content-cache hits;
below that the memo cannot recover the serialization waste and the lane
ends NO-GO. "Memoable" is the conservative count: a repeated
`(id(left), id(right), ctx_key)` whose earlier occurrence this build was
decided natively (content-cache hit or a 0/1 coded answer; code-3
consult-cut answers are excluded, matching the answer cache's own
persistence rule).

## Probe

`MYPY_SUBTYPE_IDENTITY_PROBE=1` (default off; every hook site checks the
bool, production behavior unchanged), in the style of the in-tree
`MYPY_SERIALIZE_STATS` probe. Hooked at the four points of the native
`_is_subtype` block: entry, post-serialization verify, content-cache
serve, coded decision. State resets at the same build boundaries as
`_subtype_answers` (`_clear_subtype_batch`, `_set_native_subtype_active`).
The id-set is capped (`_IDENTITY_PROBE_CAP = 1_000_000`, ~350 MB worst
case; actual use 107,875 entries, ~38 MB, overflow 0).

Design note: the issue specifies weak-identity keying (`weakref.ref`
stored per entry, verified before serving). The first probe build
implemented exactly that and died on the first native-path entry:
**mypy `Type` objects are slots-based with no `__weakref__` anywhere in
the hierarchy** (`Type.__slots__` plus `nodes.Context.__slots__`), so
`weakref.ref` raises `TypeError` for every one of them. Measured
directly: 149,710 of 149,710 native-path entries (100%) rejected the
weakref. The probe therefore keys on the raw id pair and verifies a
repeat against the pair's earlier wire bytes before counting it: equal
bytes prove the repeat (a recycled id serializes differently, and each
such event lands in the `recycled` counter), which is the same
bytes-equality contract `_subtype_answers` itself rests on.

## Measurement

Command (from the worktree root):

```
MYPY_SUBTYPE_IDENTITY_PROBE=1 MYPY_NUM_WORKERS=0 \
PYTHONPATH=/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver:/private/tmp/mypy-rs-local-typekernel \
.venv/bin/python -m mypy --config-file mypy_self_check.ini -n0 --no-incremental \
  --dump-build-stats -p mypy -p mypyc
```

| counter                    | value   |
| -------------------------- | ------- |
| entries                    | 149,702 |
| content_hits               | 127,096 |
| identity_seen              | 41,724  |
| identity_hits (memoable)   | 41,711  |
| content_hits_identity      | 41,711  |
| recycled (id reuse caught) | 84      |
| ser_failed                 | 19      |
| inserted (distinct pairs)  | 107,875 |
| overflow                   | 0       |

Rates:

- memoable identity hits / content hits: 41,711 / 127,096 = **32.8%**
- memoable identity hits / all native-path entries: 41,711 / 149,702 = **27.9%**
- `identity_hits == content_hits_identity`: every memoable repeat is a
  content-cache hit; the memo adds nothing on entries the content cache
  already misses.

The population agrees with the #1827 wire-audit baseline (149,163
lookups / 126,633 content hits there vs 149,702 / 127,096 here), so the
instrument sees the same traffic the waste was measured on.

## Decision

The rate clears the pre-registered gate (32.8% >= 30%), so the waste is
real and harvestable in principle: a sound front memo would skip the
serialization of 41,711 pairs (~1.4 MB wire by the audit's ~35
bytes/event figure). The lane still ends **NO-GO**, because the issue's
non-negotiable soundness mechanism is structurally impossible:

1. Weak identity cannot be keyed on: 0/149,710 entries weakrefable.
   Adding `("__weakref__",)` to the `Type` hierarchy's slots is the only
   way to make it possible, and that is a per-instance memory-layout
   change on the hottest objects in the checker (8 bytes each, plus
   mypyc-compiled-build implications), not a memo-local change this
   slice may make.
2. A raw-id memo without liveness verification is unsound: the probe
   caught 84 recycled-id events in this one build, each of which would
   have served a stale answer.
3. Keying on the objects (strong refs, even LRU-bounded) is what the
   issue text explicitly forbids, for exactly this RSS reason.
4. Byte-verification (what the probe does to stay honest) requires the
   serialization the memo exists to skip.

No production change. Follow-up options for the maintainer: (a) a
dedicated slice adding `__weakref__` to the `Type` slots with its own
RSS + instructions-retired A/B, which is the only sound path to
harvesting the measured 32.8%; (b) re-ranking this seam below the seams
where the waste does not need identity keying.

## Gates run

- Pre-flight: `.venv/bin/python scripts/assert_worktree_import.py .`
  from inside the worktree, exit 0, resolves to the worktree.
- Differential suites, probe off (default):
  `testsubtypes.py testtypes.py testtypes_native_engagement_types.py
  testauditwiretraffic.py` with `TEST_NATIVE_TYPE_KERNEL=1
  TEST_NATIVE_PARSER=1`, scratch PYTHONPATH, `-n0`:
  1328 passed, 2 skipped.
- Same-family suites, probe on: `testsubtypes.py
  testtypes_native_engagement_types.py` under
  `MYPY_SUBTYPE_IDENTITY_PROBE=1`: 1148 passed.
- Probe-on cold self-check: 0 errors (the measurement run above).
- Final-state cold self-check (probe off, exact corpus shape): 0 errors.
- `uvx black --check` + `uvx ruff check` on touched files: clean.
