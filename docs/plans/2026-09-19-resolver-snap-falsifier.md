# Resolver-snapshot collect profile and lever-1 falsifier (#66)

Status: measurement only, no production change. Verdict: **NO-GO** under the
pre-registered gate. Companion: issue #66 (closed by this falsifier); parent
attribution `docs/plans/2026-09-19-gap-attribution.md` (#61, merged); sibling
falsifier `docs/plans/2026-09-19-wire-churn-falsifier.md` (#63, lever 2,
same verdict).

- Head: `origin/main` = `acb2ff40d`, worktree `worktrees/mypy-rs-resolver-snap`
  (branch `perf/resolver-snapshot-delta`).
- Extensions from the standard scratch dirs
  (`/private/tmp/mypy-rs-local-{ast,resolver,typekernel}`); the shared
  `type_kernel.cpython-313-darwin.so` was built from the wire-churn branch
  `2bd8a9aaa`, whose diff touches only `wire.rs`, probes and docs —
  `typeinfo.rs`/`aliases.rs`/`subtypes.rs` are byte-identical to this head
  (verified by diff), so the snapshot/update behavior measured here is exactly
  this tree's. `scripts/assert_worktree_import.py` exit 0 before every run.
- Corpus: cold self-check, `mypy_self_check.ini -n0 --no-incremental
  -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`), 378 files,
  "Success: no issues found" on every counted run, probe exit 0.
- Evidence: `/private/tmp/resolver-snap/` — `sigs.err` and `sigs2.err`
  (determinism pair, byte-signature mode), `light.err` (timing run, no
  signature overhead), `sigs3.err` (field-level diffs and walk attribution
  of the changed entries; run exit 0, "Success: no issues found in 378
  source files").

## The pre-registered gate

From issue #66, fixed before any measurement: instrument consecutive
collects; if fewer than 50% of re-serialized snapshot entries are unchanged
between consecutive collects, the lever is NO-GO — close with the measured
rate and no production change. The 50% threshold was chosen because a
delta-serializer only pays when the re-serialized population is dominated by
unchanged entries.

The issue's premise — "the incremental resolver snapshot re-serializes the
full graph on every collect", with the #61 audit's `re_pushed_builtins:
56,138` read as the per-collect re-serialization volume — needed one
correction before the gate could fire either way: that audit counter counted
re-**collected** entries (walked again), not re-**pushed** entries (actually
serialized and fed to Rust). The falsifier measures both populations.

## What was measured

`misc/resolver_snap_phase_profile.py` wraps `_build_native_resolvers`,
`_collect_incremental`, `_split_native_pending`, `_native_ctor_blob` and
`mypy.types.write_str` in one cold self-check process. In `--sigs` mode every
collected entry gets a byte signature mirroring `snapshot_type_info`
(crates/type_kernel/src/typeinfo.rs) field-for-field — same fields, same
failure defaults, type fields through the same `Type.write` wire path — stored
per fullname and compared against the entry's previous appearance. Light mode
drops the signature work to keep timers honest.

Two populations, because the gate's wording ("re-serialized") admits two
readings:

1. **Entries actually re-serialized and fed to Rust** (the re-push set
   `_split_native_pending` produces behind the #1641 signature gate).
2. **Entries re-collected by the walk** (already snapshotted, walked again).

## Results

Cold self-check, 546 collects, 68,154 collected infos (all three
signature-mode runs agree with the #61 audit to the entry), 6,528 new,
6,773 aliases:

| measure | value |
| --- | --- |
| re-collected entries (walked again) | 61,626 (56,138 builtins + 5,488 others) |
| re-collected, signature unchanged | 61,623 / 61,626 = 99.995% |
| actual re-push feed to Rust, whole run | **1** entry |
| re-push unchanged fraction (gate population A) | 0 / 1 = **0.0000** |
| re-push bytes (mirror proxy) | 3,037 |
| `write_str` during resolver build | 26,974 of 1,719,985 total (1.6%) |

Changed entries over the whole run, with the walked module that yielded
each entry and the per-field diff (from `sigs3.err`; TypeInfos are
collected via every walked module whose symbol table references them,
so the reason names the walked module, not the defining one):

```
changed collect=113 reason=builtins:builtins repushed=True bytes 2918->3037 builtins.int
  field promote 33->152 bytes
changed collect=290 reason=in_scc:multiprocessing.util repushed=False bytes 1687->1674 logging.Logger
  field enum_members 21->8 bytes
changed collect=418 reason=in_scc:mypy.test.testmypyc repushed=False bytes 5453->5105 unittest.case.TestCase
  field enum_members 356->8 bytes
```

The single re-push is `builtins.int` at collect 113: the #1641 gate caught
genuine growth (its promote field grew 33->152 bytes as builtin promotions
accumulate), exactly the case the gate exists for. The two non-builtins
entries changed in exactly one field, `enum_members`, which is the
snapshot-staleness class the kernel already documents and routes around
(next section) — not new churn.

Light-run phase decomposition of `resolver_build_s` 0.495s over 546 collects:
first full build 0.052s + collect walk 0.125s + signature gate split 0.040s
+ ctor blobs 0.116s (+0.031s first) + update+installs residual 0.163s. The
probe's own signature work costs 0.869s and is reported separately; it
inflates `resolver_build_s` to 1.357s in sigs mode, which is why the timing
numbers come from the light run.

## Verdict

**NO-GO**, under both readings of the gate:

- Population A (what a delta-serializer would actually touch): 0% unchanged
  over the whole run — but the population is 1 entry (~3 KB), so the gate
  fires on a population too small to matter either way.
- Population B (re-collected entries): 99.995% unchanged, but these are
  already **skipped** by the #1641 signature gate — the Rust updater never
  re-serializes them. A delta-serializer layered on top would re-serialize
  entries that are currently not serialized at all; the addressable saving is
  ~3 KB per cold self-check, orders of magnitude below the lever's 3-5e9
  instruction ceiling.

What remains per collect is change *detection* (the module walk plus the
builtins signature comparison, 0.165s combined over 546 collects), not change
*serialization*. Making detection cheaper would need mutation-site hooks on
the Python TypeInfo graph, which the #1641 docstring records as not available
for the cache-fixup path; that would be a different lever with its own
falsifier.

The issue's `write_str` premise also dissolves under attribution: only 26,974
of 1,719,985 `write_str` calls (1.6%) happen inside the resolver build. The
1.57M-call gap between native-on and native-off arms is per-call wire prep in
the type kernel, not snapshot upkeep — consistent with the wire-churn
falsifier's finding that the churn is solve-side (#63).

## What ships

The probe (`misc/resolver_snap_phase_profile.py`) and this record. No
production change; the #1641 signature-gated update path stands as is.

## The changed non-builtins entries: documented staleness, no new issue

Both non-builtins entries were re-collected because an in-SCC module
*referencing* them was walked (imported names share the TypeInfo node, so
any walked module whose symbol table references a class yields it):
`multiprocessing.util` at collect 290, `mypy.test.testmypyc` at collect 418.
Both changed in exactly one field: `enum_members`.

`TypeInfo.enum_members` is a live-computed property over the class's
symbol table (mypy/nodes.py:4136), so its value depends on how far member
analysis has progressed. Both entries had transient content at first seal
(one member for Logger, a longer transient list for TestCase) that settled
to `[]` by the later re-collection. The kernel documents this
snapshot-staleness class and routes around it: enum expansion defers to
Python and live-enum/final reads go through the live typeinfo map
(mypy/build.py ~line 1896; crates/type_kernel/src/typeops.rs:899 and
:1759; crates/type_kernel/src/cond_types.rs:93). The one consumer that
trusts the snapshot's `enum_members` (`try_contracting_literals_in_union`,
crates/type_kernel/src/setops.rs:3847) is `is_enum`-gated and degrades to
a missed contraction in the documented stale direction; both measured
entries are non-enums and never reach it. No issue filed: the measured
behavior matches the documented design — this measurement confirms the
staleness is real and bounded to the documented class.

Measurement caveats, stated rather than hidden:

- Wall times are load-contaminated (sibling CI runner and a Lean lane were
  active; load average 52-78 during the runs). The counters and byte
  volumes are exact; only the second-decimal timers wobble.
- The signature mirror is Python-side: byte-identity between two appearances
  proves the *inputs* the Rust updater would read are identical, not that
  Rust stored them. A Rust-side mirror failure counter guards the mirror
  itself (0 on every counted run).
- `no-baseline` appearances (3,237) are first-seal entries stored without a
  comparison; they are not part of either gate population.
