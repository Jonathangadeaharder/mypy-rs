# Type-ownership inversion brief: first bounded slice (#72 step 1)

Status: design brief, second deliverable of this lane. No production
change here. Companion ADR: `docs/adrs/0007-rust-owned-type-representation.md`
(same PR; the ADR fixes the architecture, this brief fixes the first
experiment that makes epic #72 step 1 evaluable without a multi-quarter
program). Evidence base: #71 Step 0 (PR #78), #70 (#75), #61/#63/#66/#58,
ADR-0004/0006, `docs/plans/2026-09-19-gap-attribution.md`.

What this brief fixes before any implementation: the slice (section 1), the
design of the smallest flip (section 2), the correctness gate (section 3),
the deletion-framed progress gate (section 4), the pre-flip measurement with
its pre-registered falsifier (section 5), what cannot invert yet and the
fallbacks (section 6), the risk register (section 7), non-goals (section 8),
and the effort estimate in bounded lanes (section 9). No claim below depends
on a document that does not exist at the time of this PR; the #71 Step-0
record is cited as PR #78 (`docs/plans/2026-09-19-subtype-residency-step0.md`
on branch `perf/subtype-residency-step0`).

## 1. The slice: the immutable leaf family, default-off

First family set (all five flipped together, one gate):

| class | fields | checker-phase in-place mutators |
|---|---|---|
| `AnyType` (`mypy/types.py:1567`) | `type_of_any`, `source_any`, `missing_import_name` | none (O/O/R-for-object in the ADR-0004 inventory, `docs/adrs/0004-e1-stage-b-type-proxy-reflect-model.md:183-186`) |
| `UninhabitedType` (`:1675`) | `ambiguous` | none (`:187-188`) |
| `NoneType` (`:1735`) | unit | none (`:189`) |
| `ErasedType` (`:1781`) | unit | none (`:190-193`) |
| `DeletedType` (`:1811`) | `source` | none (`:193`) |

Why this family and not `Instance` (the highest-traffic one):

1. **No mutation coherence to build first.** The whole ADR-0006 failure
   surface — per-mutation capture, `__setattr__` interception, touch-point
   audit (ADR-0004 Decision 3,
   `docs/adrs/0004-e1-stage-b-type-proxy-reflect-model.md:324-362`) — exists
   because `Instance.args` and `CallableType` fields mutate in place
   (`instance.args = ...` at `mypy/typeanal.py:1557`, `:3296`,
   `mypy/exprtotype.py:132`; `resolved.special_sig = ...` at
   `mypy/checkexpr.py:1125`, `result.variables = ...` at
   `mypy/typeanal.py:4222`). The leaf family is immutable in the
   checking phase and in semanal, so the slice proves the representation
   mechanics (arena, facade, identity, cache isolation, daemon reset) with
   coherence reduced to immutability-by-construction.
2. **Real mint traffic.** `AnyType` is constructed ad hoc all over the
   checker (e.g. `AnyType(TypeOfAny.special_form)` in
   `mypy/checkexpr.py`) and is the most-constructed type in the corpus;
   the mint-at-construction path therefore gets exercised at production
   volume, which is what the amortization falsifier (section 5) needs.
3. **Real nested-serialization traffic.** These leaves ride inside
   `Instance.args`, unions and callables as *children*, so the family's
   deletion shows up in the funnel's child-walk counters, not only at
   top-level operand sites (the #71 Step-0 population showed the subtype
   family's own serialize split: hit 180,623 / builtin-shortcut 29,429 /
   miss 89,506 of 299,558 appearances; step0 record).

Slice shape (all behind one default-off `Options` gate, name to be fixed at
implementation, absent from `OPTIONS_AFFECTING_CACHE`):

- Rust: `TypeArena { records: Vec<TypeRecord> }` plus the `TypeId` protocol
  (ADR-0007 Decision 1), extending `identity.rs`
  (`crates/type_kernel/src/identity.rs:171`).
- Python: the five classes' data slots are removed; the constructor mints
  the arena record and stores the `TypeId` (+ the `Context` position slots,
  which stay Python-side, `mypy/nodes.py:161`). Reads route to the arena
  (ADR-0007 Decision 2).
- The serialize funnel (`_serialize_type_for_visitor`,
  `mypy/types.py:4746`) serves a flipped child from its arena record
  (encode-at-mint) instead of walking the Python object.
- **No seam signature changes in this slice.** The kernel seams keep
  receiving wire bytes; `TypeId`-taking seam variants belong to the
  Instance/Callable slices where the primary-operand traffic is. This keeps
  the blast radius at T2 (one lane's own suite + one targeted corpus file +
  engagement counters), per the AGENTS.md tier table.

## 2. The flip, mechanically

Construction: `AnyType(type_of_any, source_any, ...)` packs its fields,
calls one new kernel entry (the #42 pointed-remedy fetch pattern from day
one, `mypy/subtypes.py:120-125`, `:949-956`), and stores the returned
`TypeId`. The kernel entry decodes nothing — it receives the field values
as scalars plus optional inline blobs for `source_any`/`missing_import_name`
children — and inserts the record. Decode of a child blob uses the existing
`decode_type` path; a decode failure returns -1 and the constructor raises
the same error the byte path would (fail-early, no silent fallback).

Serve: the funnel's per-child dispatch hits a facade, probes the id table
(the shipped F3 protocol shape: dict probe + `entry is t` identity check +
tvar fingerprint, `mypy/types.py:206-225`; this lane consumes the #28 shared
lookup helper rather than adding a third copy of the policy), and returns
the record's wire bytes. `defers`/misses are byte-path, unchanged.

Reset: arena clear rides `_clear_native_resolvers` with the identity
service's preserving-reset split for daemon rechecks
(`identity.rs:33-42`, `:228-239`; call sites
`mypy/server/update.py:1128-1130`).

Gate-off is exactly today: with the option off, the classes are the
pre-existing ones, untouched; there is no probe, no mint, no interception.
The slice is additively gated like every native seam before it.

## 3. Correctness gate (differential parity, both gate states)

1. `testcheck.py` with the slice gate on and off, native
   kernel/parser/resolver on in both states (the standard differential);
   zero new failures vs baseline in both states.
2. The parity suites for the touched surface: `testtypes.py` (all
   `Native*Suite` differentials), plus `testsubtypes.py`,
   `testtypes_native_engagement_types.py`, `testauditwiretraffic.py` —
   the #71 Step-0 lane's differential set, extended with a new
   `NativeTypeArenaSuite`: facade equality/hash parity against gate-off
   results, `isinstance`/`accept`/`TypeStrVisitor` rendering parity, and
   an engagement test asserting the mint counter moves.
3. Byte-identical diagnostics: cold self-check
   (`mypy_self_check.ini -n0 --no-incremental -p mypy -p mypyc`, single
   process), gate on and gate off, `diff` of outputs; "Success: no issues
   found" in both. The self-check loads a real user plugin
   (`mypy_self_check.ini:22`, `mypy.plugins.proper_plugin`), so plugin
   visibility is exercised by the corpus itself, not by argument.
4. Fine-grained + daemon + incremental families both gate states
   (`testfinegrained`, `testmerge`, the daemon suites): the arena reset
   contract (ADR-0007 Decision 3) is tested by the suites that already
   exercise `_clear_native_resolvers`.
5. Cache round-trip: `testpythoneval`/cache suites confirm no
   `TypeId`-shaped byte reaches `mypy.cache` (plus a targeted unit test
   asserting the gate is absent from `OPTIONS_AFFECTING_CACHE`).
6. Pre-flight: `scripts/assert_worktree_import.py` exit 0 from inside the
   lane worktree before every counted run (the #1789 lesson).

## 4. Deletion-framed progress gate (pre-registered, milestone semantics)

Per the corrected rule (#72; `docs/remaining-migration-plan.md:18-27`), the
slice is NOT gated on net-positive instructions against the hybrid. The gate
is correctness (section 3) plus measured deletion:

1. **Canonicality flip**: the five classes' data fields are absent from
   their `__slots__` in the gate-on build (structural assertion in the
   suite; the Python representation of the family has no data to be
   canonical).
2. **Walk unreachability**: with the gate on, family-attributed Python
   serialize *walk* entries (the `MYPY_WIRE_PREP` ledger and the audit's
   `serialize_writes`/node-walk counters, family-attributed the way the
   #71 Step-0 lane attributed decode windows) drop to zero for
   arena-served constructions; misses (the refusal classes: pre-fixup,
   tvar-fingerprint, decode-failed mint) are reported per class and must
   be a named, counted set, not an unbounded residue.
3. **Mint accounting**: `arena_mints` == family construction count
   (measured by the section-5 probe), `arena_serves` ==
   family-attributed child appearances minus refusals; both counters
   published under `MYPY_SERIALIZE_STATS=1`.
4. **Reported, not gated**: total instructions and peak RSS both gate
   states (the /usr/bin/time -l discipline), recorded in the verdict
   record. The milestone perf gate fires only at the family's graduation
   (byte-path deletion for the family), which is a separate T3 lane.

A miss in 1 or 2 is a design failure of the slice (the flip did not move
authority); a refusal rate large enough to empty the served population is
an honest NO-GO with the measured numbers, mirroring the #71 Step-0
outcome shape.

## 5. Pre-flip measurement (the amortization falsifier)

The one number that can kill this slice shape before any production code,
fixed now: **mint-at-construction pays one kernel crossing per construction;
the byte path pays per appearance.** If constructions vastly outnumber
appearances, the flip cannot amortize even under the milestone rule, because
it adds work on a hot path without deleting its own bookkeeping.

Step 0 (measurement only, default-off probe, in the identity-probe lineage):

- Count per family, per build: constructions, first appearances at the
  funnel, total appearances at the funnel (nested + top-level), refusal
  classes. Cold self-check, single process, counters load-invariant, probe
  runs never used as instruction baselines (the #71 Step-0 probe-pin
  discipline, step0 record "Caveats").
- Pre-registered falsifier: if `constructions / appearances` for the family
  set exceeds 1.0 (each object used at most once), the mint cost is paid
  more often than the walk it replaces, and the lane reports the slice
  shape NO-GO for that family and stops — no flip lane starts. The blob
  falsifier's per-crossing floor (~139 instr/arg marshal;
  `docs/plans/2026-09-19-blob-marshal-falsifier.md:110`, plus the 1-arg
  floor 105.9 for the crossing itself, `:82`) prices the mint arm; the
  #61/#70 microbench prices the walk arm (serialize hit 1,815 / miss
  15,265-16,072 instr, step0 record).
- Expected, stated as a band and not a promise: leaf constructions are
  dominated by `AnyType` at checker volume, but leaves are also the
  cheapest walks (2-byte singleton tags, fast-path served, mostly F3
  hits), so the per-appearance saving is small; the band is wide and only
  the probe settles it. This is precisely what makes the slice decisive
  rather than assumed.

## 6. What cannot invert yet, and the fallbacks

Stated plainly, family by family:

- **`Instance`** (`mypy/types.py:1917`): `args` is rewritten in place
  during semanal (`mypy/typeanal.py:1557`, `:3296`, `mypy/exprtotype.py:132`),
  `extra_attrs` and `last_known_value` are checker-phase facts
  (ADR-0004 inventory,
  `docs/adrs/0004-e1-stage-b-type-proxy-reflect-model.md:205-229`),
  and plugins reach `Instance`
  objects in every hook context. Inverting it requires the write-through
  facade plus the phase-scope discipline (proxies absent during semanal
  mutation) — the design exists in ADR-0007 Decisions 1-2 but its
  touch-point audit is not done. Fallback: byte path, unchanged.
- **`CallableType`/`Overloaded`** (`:2497`, `:3134`): the widest
  in-place-mutated field set in the checker (`special_sig` written at
  `mypy/checkexpr.py:1125`, `variables` rewritten at
  `mypy/typeanal.py:4222` and `mypy/applytype.py:675`; `definition`
  repairs in checkmember, ADR-0004 inventory `:264-271`). Same status:
  byte path until the write-through lane.
- **`TypeVarLikeType` family** (`:884`, `:1025`, `:1194`): `meta_level`
  mutates in place during inference (the tvar-fingerprint poison,
  `mypy/types.py:143-147`) — the exact hazard the F3 refusal rules exist
  for. Byte path until the fingerprint discipline is re-grounded against
  arena records.
- **`UnionType`, `TupleType`, `TypedDictType`, `TypeType`, `TypeAliasType`,
  `UnboundType`, `Parameters`, `LiteralType`**: container/discriminated
  families; inverting any of them needs child-representation mixed-graph
  rules exercised first (ADR-0007 Decision 6) — they follow the leaf slice,
  in traffic order, each its own gated lane.
- **`mypy/nodes.py` entirely**: epic step 2, out of scope. The `Context`
  position slots stay Python-side even for flipped families
  (ADR-0007 Decision 2).
- **Runtime-only wrappers** (`TypeGuardedType`, `RequiredType`,
  `ReadOnlyType`, `PartialType`, `PlaceholderType`; ADR-0003 set 2): never
  minted; they never reach the wire today and need no arena record. A seam
  seeing one defers exactly as now.

The fallback for every un-flipped consumer is the status quo byte path:
nothing in this slice changes any un-flipped family's code, cost, or
semantics. Plugin-visible mutability is the hard driver that keeps
`Instance`/`CallableType`/`TypeVar` families on that fallback until their
own write-through lanes land.

## 7. Risk register

1. **Facade read routing is net-negative** (the strongest counter-evidence:
   ADR-0006 arm-2, 17.43M routed `args` reads, strictly negative,
   `docs/adrs/0006-instance-replacement-view.md:249-256`). For the leaf
   family the read volume is far smaller (plugin/visitor reads only; no
   `args`-hot container), and coherent scalar caching is available since
   the families are immutable — but this is measured in the lane, not
   assumed. Mitigation if it regresses beyond the recorded-not-gated
   budget: per-field read-through caches for scalars (coherent by
   immutability), decided by measurement, or the slice reports the cost
   honestly at graduation time.
2. **Mint amortization fails** (section 5 falsifier). Mitigation: the
   pre-registered probe kills the lane before the flip.
3. **Mixed-graph staleness** (a minted record's inline child blob goes
   stale because an un-flipped child mutated after mint). Mitigation: the
   F3 acceptance predicate governs what may be minted; refusals fall back
   to the byte path, so the failure shape is a miss, never a stale byte
   (ADR-0007 Decision 6; the ADR-0006 serve-table precedent,
   `docs/adrs/0006-instance-replacement-view.md:64-77`).
4. **Daemon/astmerge identity**: preserved facades must keep serving after
   a recheck. Mitigation: the preserving-reset protocol is shipped
   (`identity.rs:33-42`); the fine-grained/daemon suites gate both states
   (section 3.4).
5. **Cache leak of `TypeId`s**: impossible by construction (thread-local
   mint, never serialized; ADR-0007 Decision 3), and asserted by test
   (section 3.5). The `__reduce__` raise keeps a future pickle consumer
   from smuggling one.
6. **Stale-kernel ABI** (#42): the new mint entry adopts the
   pointed-remedy fetch in its introducing commit; the pre-existing gap on
   old entries stays #42's own fix and is not widened by this lane.
7. **Lookup-path drift** (#28): the funnel's arena probe consumes the #28
   shared lookup helper; the lane does not add a third validation policy.
   (If #28's helper has not landed by then, extracting it is lane step 1
   and closes #28, same as the #71 brief specified.)
8. **mypyc coexistence** (ADR-0001/E3): slots/descriptor changes under a
   mypyc-compiled build are unexercised locally. Mitigation: gate-off is
   byte-identical to today (no class change is active), and the gate-on
   mypyc answer is recorded as an open assessment per family, not claimed.
9. **RSS growth from arena + pins**: pre-measured by the section-5 probe
   (record count x per-record size), reported in the verdict; the ADR-0006
   pin lesson (+501 MB at 1.4M entries) is the scale reference.
10. **Third-party `Type` subclass break** (slot layout change under the
    gate): accepted and documented (ADR-0007 Decision 2); no in-tree
    consumer exists.

## 8. Explicit non-goals

- No shadow or mirror of un-flipped families; no dual-write capture; no
  read-flip shadow (the F1/G4/ADR-0006 shapes are structural non-goals,
  same list as the #71 brief, `docs/plans/2026-09-19-subtype-residency-brief.md:535-550`).
- No bytes-keyed answer cache, in any guise.
- No `Instance` replacement view reopened (ADR-0006 is Rejected and
  deleted; this slice touches leaf classes only).
- No seam signature changes, no kernel-side answer caches, no
  `TypeId`-taking seam variants (later slices).
- No default flip; no `CACHE_VERSION` change; no change to
  `OPTIONS_AFFECTING_CACHE`.
- No plugin API change, no `mypy/nodes.py` change, no semantic-analysis
  ownership work (epic steps 2-4).
- No `__weakref__` addition anywhere (ADR-0007 Decision 5: nothing needs
  it; if a consumer appears, that is its own slice with an A/B).

## 9. Effort estimate (bounded lanes)

- **Lane 0 (this PR):** ADR-0007 + this brief. Done here.
- **Lane 1, Step 0 (measurement only, no production change):** the
  construction-vs-appearance probe (section 5), default-off, one
  measurement lane, corpus-class runs only. Can end the slice shape on its
  own numbers. Estimated: one lane, comparable to the #71 Step-0 lane.
- **Lane 2 (the flip, default-off):** `TypeArena` + `TypeId` extension of
  `identity.rs`, the five facades, the #28 helper consumption, the funnel
  serve branch, the mint entry with the #42 fetch pattern, counters, and
  the `NativeTypeArenaSuite` + differential suites both gate states.
  Estimated: 2-3 build-class lanes; T2 blast radius (one family set, one
  funnel branch, zero seam-signature changes).
- **Lane 3 (the verdict):** the section-3/4 battery, the counter
  arithmetic, the recorded instruction/RSS report, the record doc.
  Deferred regardless of verdict: any second family, any seam variant,
  any default flip (its own T3 lane), the mypyc assessment, and any
  read-through cache (measurement-gated optimization, not scope).
