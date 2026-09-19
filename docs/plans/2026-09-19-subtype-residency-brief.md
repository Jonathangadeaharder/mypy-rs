# Subtype residency design brief: Rust-resident subtype family with stable TypeIds (#71)

Status: design brief, first deliverable of #71. No production change in this
lane. Companion evidence: the gap attribution (#61,
`docs/plans/2026-09-19-gap-attribution.md`), the identity-repeat probe (#58/#59,
`docs/plans/2026-09-19-perf-subtype-identity-probe.md`), the two falsifiers
(#63, #66), ADR-0006 (#54), the research report
(`docs/reports/research-report-mypy-rs-rust-parity-2026-09-19.md`). Counts
below are stamped on heads `5a2c30466` (probe) and `8afb1d4ac`/`acb2ff40d`
(attribution/falsifiers); every implementation lane re-measures on its own
head before trusting them.

What this brief fixes, before any implementation: the family and its
boundaries (section 1), the TypeId protocol and its invalidation contract
(section 2), the counter-defined wire-deletion requirement (section 3), the
Step-0 B measurement (section 4), the pre-registered acceptance gate and its
paired-run design (section 5), the risk register (section 6), the slicing
(section 7), and the non-goals (section 8). One conflict with the gate as
written is already visible from code reading and probe arithmetic; it is
recorded in section 3, not redesigned around.

## 1. Family and slice definition

The closed subtype subgraph, centered on the production crossing
`rust_is_subtype_coded` (`crates/type_kernel/src/subtypes.rs:4490`):

**In the slice:**

- The native block of `mypy/subtypes.py:_is_subtype` (`mypy/subtypes.py:899-995`):
  the two operand serializations (`mypy/subtypes.py:922-923` via
  `_serialize_type`, `mypy/subtypes.py:297-328`), the dedup-key build and
  `_subtype_answers` lookup/persist (`mypy/subtypes.py:929-942`,
  `mypy/subtypes.py:982-991`), and the coded crossing
  (`mypy/subtypes.py:949-973`).
- The Rust coded entry `rust_is_subtype_coded` and the `decode_type` pair it
  runs per call (`crates/type_kernel/src/subtypes.rs:4490-4548`,
  `crates/type_kernel/src/subtypes.rs:3875-3885`).
- The answer cache `_subtype_answers` (`mypy/subtypes.py:399`), keyed on
  `(left_bytes, right_bytes, ctx_key)` with the 9-flag context key
  (`mypy/subtypes.py:516-534`).
- The family's share of the serialize funnel and the F3 wire cache: the
  wire-prep ledger books the family at two `subtypes._is_subtype` sites,
  148,874 + 148,873 entries, 0.283 s + 0.185 s consumed, and 0.243 s of the
  0.244 s total unconsumed prep in a build (attribution doc, "Wire-prep
  ledger"). One operand serialization per operand per lookup: every lookup,
  repeat or not, rebuilds both key byte strings today.
- New machinery this design adds: a per-build TypeId handle table (Python
  side), a resident tree store and TypeId-keyed answer store (Rust side), and
  one new kernel entry (section 2).

**Out of the slice:**

- `SubtypeVisitor` and every `-1` defer path (`mypy/subtypes.py:995`): the
  Python fallback is untouched and remains the correctness net.
- The scalar `rust_is_subtype` (`crates/type_kernel/src/subtypes.rs:4424`,
  parity-only) and `rust_is_subtype_batch`
  (`crates/type_kernel/src/subtypes.rs:4563`, retired accumulator, #33).
- The pre-serialization fast paths and right-side dispatch
  (`mypy/subtypes.py:811-897`): they run before the native block and are
  unchanged.
- Every other consumer of `_serialize_type` (meet, solve, checkmember,
  constraints, and the rest of the 305 called seams). The TypeId protocol is
  designed to be extensible to them, but this experiment flips nothing outside
  `_is_subtype`.
- The protocol consult-cut machinery and `_subtype_decode_cache`
  (`mypy/subtypes.py:333-364`, `_restore_protocol_member_definition`
  `mypy/subtypes.py:367-389`): consult work is engine cost, present in both
  arms, not boundary cost. It is excluded from B for the same reason solving
  is.
- The F3 wire cache itself (`mypy/types.py:136`, `_write_type_cached`
  `mypy/types.py:5619`): it stays for all other consumers. For this family
  its probe is bypassed on served lookups (section 3), not deleted.
- The identity probe (`MYPY_SUBTYPE_IDENTITY_PROBE`,
  `mypy/subtypes.py:401-491`): stays an env-gated instrument. The Step-0
  probe (section 4) extends it; production behavior is unchanged.

**Measured populations** (cold self-check, single process, identity probe):

- 149,702 native-path entries; 127,096 `_subtype_answers` content hits (85%);
  41,711 memoable identity repeats (32.8% of content hits, 27.9% of entries);
  107,875 distinct `(id, id, ctx)` pairs; 84 recycled-id events caught by
  byte verification; 19 serialization failures (probe doc, measurement table).
- The repeat population splits into two classes with different fates under
  this design. Class A, same-object repeats: at most 41,711 whole-pair repeats
  where both operands are objects the build has seen before; TypeId residency
  deletes their serialization outright. Class B, fresh-object byte-repeats:
  127,096 − 41,711 = 85,385 content hits whose operand objects were new to
  the process even though their bytes matched an earlier decision. Those
  objects are largely manufactured by the kernel's own deser-back churn (89k
  rebuilds, 206k fixup walks, 468k `copy.copy`, attribution doc
  "Deser/rebuild back to Python objects") and by ad-hoc instance creation on
  the call path. Serialize-at-most-once per object (the issue's own design
  requirement) cannot delete a first serialization: a genuinely new object
  must be walked once in any architecture that leaves Python canonical.
  Class B is therefore capped for this experiment, and the cap is measured
  before the flip (sections 3 and 4).

## 2. TypeId protocol

**Obtaining a TypeId (serialize-at-most-once).** A new kernel entry
`rust_subtype_type_id_insert(obj, blob) -> i64` is the only mint path. Python
serializes the operand exactly once through the existing funnel
(`_serialize_type`, unchanged bytes, unchanged F3 interaction) and hands the
blob across; Rust decodes it (`decode_type`, `crates/type_kernel/src/subtypes.rs:3875`),
stores the decoded `wire::Type` tree, pins the Python object, and returns a
fresh TypeId. Decode failure returns −1 and the family defers on that operand
(the byte path, unchanged). The TypeId is the stable-handle value from the
existing identity service (`crates/type_kernel/src/identity.rs:171`,
`handle_for_stable`): one handle namespace shared with the mirror, no second
identity mint, guarantee 2 (idempotent per live object) inherited for free.

**Resolution on later calls.** Python keeps the handle table:
`_type_id_table: dict[int, tuple[Type, int, _TvarFingerprint | None]]`, keyed
`id(t)`, exactly the shape and protocol of the shipped F3 wire cache
(`mypy/types.py:133-136`). A probe checks the dict, verifies `entry[0] is t`,
and returns the TypeId. Hit cost is a dict probe plus an identity check, the
same operations `_type_wire_cache_hit` performs today
(`mypy/types.py:206-225`); no serialization, no crossing, no bytes.

**What Rust stores.** Per TypeId: the decoded `Arc<wire::Type>` tree (owned
for the lifetime of the check, the issue's requirement), fetched by id on
coded calls with no decode. No Python object graph is mirrored; no mutation
capture exists (contrast F1's dual-write and G4's node mirror, section 6).
The id-keyed answer map lives Python-side v1 (see "Answer store" below).

**Lifetime scoping.** Per build, not per SCC and not per check invocation.
The resident store is cleared at exactly the boundaries that clear
`_subtype_answers` today: BuildManager construction
(`mypy/build.py:1077-1089`, `_clear_subtype_batch`) and
`_set_native_subtype_active` (`mypy/subtypes.py:137-148`), which already
resets the answer cache at manager init. Per-SCC reset is wrong for the same
reason it is wrong for `_subtype_answers`: the resolver snapshot is monotone
(first-seal-wins plus builtins re-snapshot,
`crates/type_kernel/src/typeinfo.rs:1198-1239`), so a byte-derived answer or a
resident tree stays valid across SCCs within one build and dies at the build
boundary. Daemon recheck clears the table through the existing
`_clear_subtype_batch` callers (`mypy/build.py:1089`, wirefixup, the daemon
reset chain at `mypy/build.py:1998`).

**Invalidation contract.** The failure mode that killed the #58 identity memo
was keying on identity without liveness: `Type` is slots-based with no
`__weakref__` anywhere in the hierarchy (measured: 149,710 of 149,710
native-path entries reject `weakref.ref`, probe doc), and a raw-id key is
unsound (84 recycled ids in one build, caught by byte verification,
`mypy/subtypes.py:471-473`). This design is a strong memo, not a weak one:

1. **Freed object.** Impossible to miss: the table entry holds a strong
   reference (`entry[0]`), so `id(t)` cannot be recycled while the entry
   exists, and every probe re-verifies `entry[0] is t` before serving. This
   is the F3 wire cache's exact shipped protocol (`mypy/types.py:136`,
   docstring "Strong ref to Type prevents id() reuse after GC"); the pin
   makes recycling unobservable, where #58's unpinned raw ids made it
   observable 84 times. The residual is the same one the F3 cache and identity
   guarantee 5 already document (`crates/type_kernel/src/identity.rs:33-42`):
   an object retained only by the table survives to the build boundary.
2. **Mutated object.** The table adopts the F3 cache's acceptance predicate
   whole, because the F3 cache is the shipped proof that this predicate is
   safe during the phase where it is enabled: (a) the phase gate, type
   checking only (`_set_type_wire_cache_enabled` from
   `mypy/build.py:4806-4808`; semanal/typeanal mutation is excluded by
   construction, `mypy/types.py:138-141`); (b) the pre-fixup exclusion
   (`type_ref is not None` instances are never stored,
   `mypy/subtypes.py:322-327`, `mypy/types.py:5698`); (c) the tvar
   fingerprint: entries with `fp is None` serve unconditionally, entries with
   a fingerprint serve only while `_tvar_fingerprint_valid(fp)` holds
   (`mypy/types.py:198-203`), and a fingerprint failure deletes the entry and
   downgrades to a re-serialize plus fresh mint, never stale bytes
   (`mypy/types.py:206-225` documents the identical downgrade). Inference
   mutates `TypeVarId.meta_level` in place and rebinds `.id`
   (`mypy/types.py:143-147`); the fingerprint is the mechanism that catches
   it, and it is reused verbatim, not reinvented.
3. **Build/daemon boundary.** Unconditional table clear at the
   `_subtype_answers` boundaries (above). A TypeId never survives the build
   that minted it; a daemon recheck mints fresh ids. Because the ids are
   process-local u64s from a thread-local registry (identity guarantee 1,
   `crates/type_kernel/src/identity.rs:10-12`), no cross-process or on-disk
   consumer can ever see one.

**#42 (stale-kernel ABI handshake).** The two new kernel entries are fetched
through the module attribute with the pointed `RuntimeError` + remedy shape
from the start (`mypy/subtypes.py:949-956`,
`STALE_TYPE_KERNEL_REMEDY` at `mypy/subtypes.py:122-125`). #42's gap is that
the remedy covers only `rust_is_subtype_coded`; this lane does not widen the
old entries' coverage (that stays #42's own fix), but it does not add a third
uncovered fetch: both new entries get the pattern in the same commit that
adds them.

**#28 (two wire-cache lookup paths).** The residency mint and probe would
otherwise be a third copy of identity-plus-fingerprint validation, which is
the exact drift #28 objects to. This design instead extracts the one lookup
helper #28 specifies (returning `(bytes, fingerprint) | None`, single
validation and stat policy) and makes the F3 funnel probe, the splice path,
and the TypeId mint/probe all consumers of it. #28 is closed by this lane
rather than deferred behind it; the TypeId probe is a consumer of the shared
policy, never a second policy.

**Answer store.** `_subtype_answers` is re-keyed from
`(left_bytes, right_bytes, ctx_key)` to `(left_id, right_id, ctx_key)`. V1
keeps it Python-side (a served repeat then costs no crossing at all), with
Rust fetching the resident trees by id on coded calls. Soundness is the
table's, not the key's: every id in a served key was verified against a
pinned, identity-checked entry on this very lookup, which is exactly the
verification #58's raw-id memo lacked (84 recycled ids,
`mypy/subtypes.py:471-473`) and could not add (no `__weakref__` to make a
weak memo of). This is not the experiment #71 excludes: a bytes-keyed Rust
cache still requires Python to serialize before Rust can look anything up;
an id-keyed store is served after Python has proven, without serializing,
that the operands are the same objects it registered. The consult-cut
exclusion travels unchanged: code 3 (true via consult cut) is never
persisted (`mypy/subtypes.py:988-991`,
`crates/type_kernel/src/subtypes.rs:4540-4544`).

## 3. Wire-deletion requirement

**The call-path change.** Gate on, the native block of `_is_subtype` no
longer contains a serialization for served lookups. Per lookup: resolve
`left`/`right` through the handle table (dict probe, identity check,
fingerprint check for `fp`-bearing entries); on hit for both operands, probe
the id-keyed answer map, and on a hit return with no crossing at all. On an
answer miss, make one crossing `rust_subtype_ids_coded(left_id, right_id,
<same 9 ctx flags>, resolver, infer_unions) -> i8`, which runs `is_subtype`
on the stored trees (no decode). On any table miss (first-sight object,
fingerprint failure, pre-fixup instance, decode-failed insert): serialize
once, mint, and proceed as today. The `_subtype_answers` byte-key build,
the two `_serialize_type` calls, and the Rust-side `decode_type` are
unreachable for served lookups, not shadowed: the serving path does not
contain them, and there is no capture machinery anywhere in the loop (no
`__setattr__` interceptor, no dual write, no mirror tax; contrast
ADR-0006's registration-per-mutation and G4's capture).

**Proof of unreachability (counters, pre-registered before the flip):**

- `MYPY_WIRE_PREP=1` ledger: entries at the family's two
  `subtypes._is_subtype` sites must drop from 297,747 to
  (2 × distinct-object first sights + excluded classes). Reported alongside:
  the repeat-class elimination ratio, below.
- A new set of residency counters: `resolve_hits` / `resolve_misses` per
  operand (with `fingerprint_reject`, `pref_fixup_reject`, `insert_failed`,
  `inserts`), and `answer_hits` / `ids_coded_calls` (answer-map serves vs
  crossings that reached the engine).
- Gate-1 metric, operationalized: baseline repeat-class entries are those
  whose operand objects have >=2 native-path appearances in the build
  (measured by the Step-0 probe, section 4); the counter ratio
  `repeat-class entries served without serialization / baseline repeat-class
  entries` must be >= 0.95. Total family funnel entries drop by the
  complement of the first-sight rate, reported but not gated.
- Family-attributed decode: with the mode-1 `WirePhaseCounters`
  (`crates/type_kernel/src/wire.rs:3558-3583`) plus a family attribution
  counter, decode must happen once per minted object, not once per coded
  call.

**A conflict already visible, recorded not redesigned.** Gate 1 as
pre-registered reads ">=95% of repeat serialization/decode for the family
disappears". Two readings exist, and the code reading plus probe arithmetic
bids them apart:

- Reading 1 (the operationalization above): >=95% of the repeat-class wire
  disappears. Achievable if exclusions (tvar-fingerprint churn, pre-fixup
  instances) are rare among repeats; Step 0 measures the exclusion rate and
  kills the lane before the flip if it exceeds the headroom.
- Reading 2: total family wire (all funnel entries) drops >=95%. Whether
  this reading can fire is not settled by pair arithmetic, and the brief
  will not pretend it is. Serialize-at-most-once per object mandates a
  first serialization for every distinct operand, so the total-entry
  deletion fraction is 1 − D/299,404 where D is the distinct operand count.
  Whole-pair repeats bound D only from above: at most
  149,702 − 41,711 = 107,991 first-sight objects per side (215,982 total),
  so deletion is at least ~28% in every scenario; there is no arithmetic
  lower bound on D (combinatorial object reuse could in principle push
  deletion toward 95%). What the evidence weighs against that happy case
  is the entry-cost mix: the family's ledger total (0.711 s ≈ 3.9-4.2e9
  instr over 297,747 entries, ≈ 14k instr/entry) sits near the miss class
  of the microbench (15.3-16.1k instr for a fresh-object walk vs 1.8k for
  a wire-cache hit), which implies most family entries are fresh-object
  walks and D is large, pinning the plausible deletion near the 28% floor.
  The 85,385 fresh-object byte-repeats corroborate (section 1, class B).
  But the mix inference runs through the ledger-to-instruction blend, so
  it is evidence, not measurement: the Step-0 probe measures D directly
  and settles both readings before any flip. If D turns out large, reading
  2 cannot fire in any sound architecture, including the one the issue
  itself specifies: the fresh objects are largely manufactured by the
  kernel's own deser-back churn (89k rebuilds, 468k `copy.copy`,
  attribution doc), and deleting their first walk requires the Python
  graph to stop producing fresh equal objects, which is deser-back
  avoidance (attribution lever 3), a different lever with its own
  falsifier, not this experiment.

The implementation lane runs Step 0 first precisely to size both readings'
populations; if reading 1's exclusion rate or the operand-level recurrence
rate shows the repeat-class population is too small to carry gates 1 and 2,
the lane reports the gate as unable to fire as written, with the measured
numbers, and does not silently substitute a softer metric. That report is a
valid outcome of the falsifier; #71's on-failure clause governs it.

## 4. B measurement plan (Step 0, before any implementation)

**Definition (fixed now).** B = the family's recurring end-to-end instruction
cost per cold self-check: Python serialize prep attributable to the two
`subtypes._is_subtype` wire-prep sites (including the dedup-key builds and
dict upkeep booked at those sites), crossing marshal for the coded call, and
Rust decode of the family's operands. Deser-back is zero for this family
(the coded entry returns an i8); consult-cut deser (`_deserialize_type`,
`mypy/subtypes.py:340-364`) is engine work, present in both arms, and is
excluded. B is the full family cost, all classes; the repeat-class sub-B is
measured and reported next to it because the gates act on different
fractions of each.

**Corpus and discipline.** Cold self-check, `mypy_self_check.ini -n0
--no-incremental -p mypy -p mypyc`, single process (`MYPY_NUM_WORKERS=0`),
`/usr/bin/time -l` instructions as the primary currency (load-invariant:
three rounds at load1 35-52 agreed to 0.5%, attribution doc), paired
interleaved arms, wall seconds recorded but never used for a conclusion.
`scripts/assert_worktree_import.py` exit 0 from inside the tree before every
run; every counted run must end "Success: no issues found". Extensions from
the standard scratch dirs; the probe is env-gated and default-off.

**Components and how each is measured:**

- B1, Python serialize prep: the wire-prep ledger clocks at the family's two
  sites (0.283 s + 0.185 s consumed, 0.243 s of 0.244 s unconsumed on the
  attribution head), converted two independent ways that must agree within
  15% (the #61 precedent for two independent estimates): (a) the ledger
  blend, 3.45 s ↔ ~20.6e9 instructions gives ~5.9e9 instr/s, so 0.711 s ≈
  4.2e9; (b) an event-count model, 297,747 entries × the microbench mix
  (serialize hit 1,807.8 instr, miss 15,329-16,116 instr, attribution
  microbench table) using the family's own hit/miss split measured by a new
  audit counter rather than the global 54% hit rate. The family's per-entry
  average today (~9.1k instr by the blend) already says the mix is
  miss-heavy.
- B2, crossing marshal: 22,606 coded calls (149,702 entries − 127,096
  content hits) × 4,411 instr (12-arg coded call, microbench) ≈ 0.10e9.
- B3, Rust decode: family-attributed `decode_calls`/`decode_nodes` deltas
  (mode-1 `WirePhaseCounters` plus a family attribution counter, since the
  global counters mix all 305 seams) × ~185 instr/node (#63 bench,
  `docs/plans/2026-09-19-wire-churn-falsifier.md`), ≈ 0.03-0.08e9.
- B4, deser-back: 0 for this family (i8 return).

**Expected band.** B ≈ 4.3e9, band 3.5-5.5e9 instructions, i.e. 4-7% of the
+82.4e9 kernel gap. Independent cross-check: attribution lever 4 (the
`_is_subtype` dedup-key serialization alone) was ceilinged at 2-4e9 with
0.47 s consumed + 0.24 s unconsumed per build; B adds the crossing and decode
terms to the same ledger sites, so 3.5-5.5e9 is the consistent envelope.

**The decisive pre-flip probe (extends the identity probe).** The existing
probe counted pair-level identity repeats (41,711). The Step-0 probe adds
operand-level distinctness: per operand appearance, whether the object id
has appeared before on the native path (bytes-verified against recycling,
the probe's existing discipline, `mypy/subtypes.py:453-474`), and the
storable class of each repeat (tvar-fingerprinted, pre-fixup instance,
plain). This yields, before any implementation: the distinct-object count
(which bounds both the deletion ceiling of section 3 and the resident-store
RSS), the repeat-class population (which gates 1 and 2 act on), and the
exclusion rate among repeats (which can kill reading 1 outright).

**What would falsify the B estimate:**

- The two conversion routes disagreeing by more than 15%.
- A family hit/miss mix far from what the blend implies (the event model
  corrects B1; the blend number is then reported with both routes' spread).
- Head drift: all counts above are stamped on earlier heads; the lane head
  re-measures, and a repeat-rate shift moves B and the gates together.
- The Step-0 probe itself: if operand-level recurrence is much lower than
  pair-level arithmetic suggests, the addressable sub-B shrinks below the
  gate-2 threshold (0.5 × B) and the experiment is NO-GO on measurement
  before a line of production code is written. Current honest projection,
  in per-class terms: a served lookup (both operands resident) replaces
  today's repeat-path cost (two F3-hit probes ≈ 3.6k instr plus the ctx-key
  build, tuple build, bytes hashing and answer-dict probe; the e2e repeat
  microbench puts the whole native repeat call at 28,009 instr) with two
  table probes plus either one answer-store crossing (~4.4k instr) or, if
  the answer map is kept Python-side keyed on the pinned ids, no crossing at
  all; call it ~23-27k saved per served lookup. The served-lookup count is
  bounded below by the 41,711 whole-pair repeats and above by the 127,096
  content hits, so projected I0−I1 is roughly 1.0-3.4e9 before subtracting
  new costs (table probes on every lookup, mint crossings, Rust store
  upkeep, ≈ 0.1-0.2e9), a conversion ratio of roughly 0.2-0.75 against
  B ≈ 4.3e9. The entry-mix evidence (section 3) leans the served-lookup
  count toward the 41,711 floor, i.e. toward a ratio of ~0.25, below the
  gate-2 threshold. That is not a prediction of failure; it is the
  statement that the experiment is genuinely decisive, and that the
  distinct-object count measured in Step 0 is the number that decides it.

## 5. Acceptance gate (verbatim from #71) and paired-run design

The pre-registered acceptance gate, fixed in #71 before this brief; restated
verbatim, not weakened:

> 1. **>=95%** of repeat serialization/decode for the family disappears (the
>    wire path is deleted, not shadowed).
> 2. **(I0 - I1) / B >= 0.5**: the ownership version recovers at least 50% of
>    B as an end-to-end instruction reduction on the cold self-check (I0 =
>    current kernel-on, I1 = ownership-on).
> 3. Kernel-on instructions decrease in **every** paired run.
> 4. Diagnostics remain byte-identical.
> 5. Peak RSS increases by **no more than ~10%** of baseline.

And the issue's decisive-number clause: "The decisive number is the **50%
conversion ratio**: a clean high-reuse family whose wire is physically
deleted yet converts less than half of the eliminated boundary work into
savings proves hidden capture/lookup/lifetime/allocator costs replace what
was removed, and scaling ownership further will not rescue 82.4e9. On
failure: close the ownership route unless a qualitatively different
ownership architecture is proposed."

**Paired-run design.** Arms: I0 = production kernel-on with residency
default-off; I1 = kernel-on with the residency gate on. The comparison is
against kernel-on, not kernel-off (gate 2's own definition). Three
interleaved paired rounds minimum on one head and one extension build,
single process, `/usr/bin/time -l` instructions and peak RSS per round; if
the round spread exceeds 1%, add rounds until the median is stable
(attribution discipline). Load is recorded per round; instructions and
counters are the load-invariant currency. Every counted run ends with the
byte-identical diagnostics check: `diff` of the two arms' outputs on the same
corpus, plus "Success: no issues found in 378 source files" in every run.

**Counter set per round:** `MYPY_WIRE_PREP` (family sites),
`MYPY_SERIALIZE_STATS=1` audit (`misc/audit_wire_traffic.py`), mode-1
`WirePhaseCounters` with family attribution, the new residency counters
(section 3), and the Step-0 probe populations re-measured on the same head.

**Differential suites that must stay green, both gate states:** `testcheck`
(`TEST_NATIVE_TYPE_KERNEL` on and off), the native engagement suites
(`testsubtypes`, `testtypes_native_engagement_types`,
`testauditwiretraffic`), the fine-grained family (`testfinegrained`,
`testmerge`, the daemon suites), and the cold self-check itself, which loads
a user plugin (`mypy_self_check.ini:22`, `mypy.plugins.proper_plugin`),
covering the plugin-visibility risk with a real plugin. The mirror stays
off in both arms (production default); the residency lane must not silently
measure with capture machinery armed anywhere.

## 6. Risk register

Each entry names the contract ground that killed an earlier attempt, and
this design's answer or its acceptance.

1. **Plugin visibility of `mypy.types` objects.** ADR-0006 exercised all four
   ADR-0004 contract surfaces and measured them satisfiable; plugins receive
   live `Type` objects and the residency design changes no object, class, or
   attribute a plugin can reach (it changes which bytes are never built).
   Gate: the proper_plugin corpus in every self-check leg (section 5).
2. **`__slots__` / no `__weakref__`.** The #58 impossibility stands
   (149,710/149,710 rejections). This design does not add `__weakref__` to
   the `Type` hierarchy (the #58 follow-up (a) RSS+instr A/B is explicitly
   not taken) and does not need it: the strong-pin protocol is the shipped
   F3 protocol (section 2). Consequence accepted: pinned objects survive to
   the build boundary. RSS impact is bounded by gate 5 and pre-measured in
   Step 0 (the pins duplicate references the F3 cache already holds on the
   same population, since every minted operand is serialized through the
   funnel first and becomes an F3 root, `mypy/subtypes.py:322-327`; the
   incremental cost is table entries plus Rust trees, estimated 10-40 MB at
   the plausible distinct-object counts, worst case measured before the
   flip).
3. **astmerge.** The table keys are Python objects, not `TypeInfo`
   identities; a merge that preserves object identities preserves entries,
   and a re-homed `TypeInfo` does not invalidate an `Instance` whose wire
   bytes carry only the fullname (the same argument ADR-0006 recorded, and
   the same envelope `_subtype_answers` already lives in). Daemon reset
   clears the table unconditionally at the recheck boundary (section 2).
4. **Daemon and incremental semantics.** Reset discipline is copied from
   `_subtype_answers` call-site for call-site (`mypy/build.py:1077-1089`,
   `mypy/build.py:1998`), so no state crosses a build boundary that the
   byte-keyed cache does not already cross. Gate: fine-grained and daemon
   suites both gate states (section 5).
5. **On-disk cache.** A TypeId must not leak into the cache, and cannot: it
   is a process-local u64 from a thread-local registry (identity guarantee
   1), never serialized into any blob, and the residency gate is absent
   from `OPTIONS_AFFECTING_CACHE` with `CACHE_VERSION` untouched until the
   gate graduates; the lane ships default-off and flips no default.
6. **Stale-snapshot classes (`enum_members` and documented staleness).**
   Unchanged and out of the slice: the resident trees are operand trees, not
   `TypeInfo` snapshots; the kernel's live typeinfo map and consult routing
   stay exactly as the resolver-snap falsifier documented them
   (`crates/type_kernel/src/typeinfo.rs:1281-1291`; falsifier doc, "The
   changed non-builtins entries").
7. **#42 stale-kernel ABI.** Both new entries adopt the pointed-remedy fetch
   from day one (section 2). The remaining old-entry gap stays #42's.
8. **#28 lookup-path drift.** Closed in this lane by the shared
   validation-helper extraction (section 2); the TypeId probe consumes the
   one policy.
9. **F1 dual-write capture (+78.7 s median, +85%) and G4 capture (+23%
   instructions).** The failure shape was per-object, per-mutation
   bookkeeping running alongside a live wire path. This design has no
   capture at all: minting is lazy, happens exactly where bytes were being
   built anyway, and costs nothing per object that is not touched by a
   lookup. The per-lookup costs (table probe on every lookup, mint crossing
   on first sight) are the "hidden lookup/lifetime costs" gate 2 exists to
   price; if they eat the savings, the gate fires and the route closes on
   evidence.
10. **Rust-side RSS from resident trees.** Real and pre-measured: Step 0
    yields distinct-object count and per-tree node count (bytes ÷ ~15.5
    bytes/node from the #63 counters), giving the store's RSS before the
    flip. Mitigation inside the issue's fixed design: `Arc` substructure
    sharing for repeated subtrees. If gate 5 fails on the store alone, that
    is a gate failure, not a reason to switch to storing wire blobs (that
    would be a redesign and is not taken silently; it would need its own
    issue against the "decoded representation" requirement).
11. **Consult cuts.** Code-3 answers stay unpersisted in the TypeId-keyed
    store, the same exclusion the batch flush removal applied
    (`crates/type_kernel/src/subtypes.rs:4540-4544`).
12. **The fresh-object churn cap (the recorded conflict).** The 85,385
    byte-equal fresh-object content hits are largely self-manufactured
    deser-back churn (attribution lever 3). This experiment does not address
    them and must not be quietly judged on them: Step 0 sizes the class, the
    gate counters report it separately, and if the class caps the deletion
    below the gate's thresholds, the lane reports the gate as unable to
    fire as written (section 3).

## 7. Effort estimate and slicing

The gate must be evaluable by a bounded set of lanes, not a program. All
production changes are default-off and concentrated in one family.

- **Lane 0 (this document):** the brief. Done here.
- **Lane 1, Step 0 (measurement only, no production change):** the B battery
  plus the operand-level distinctness/exclusion probe. Extends the identity
  probe (`mypy/subtypes.py:437-491`) rather than adding a second instrument;
  ships the probe default-off and the B record. This lane alone can end the
  experiment: if the repeat-class population or exclusion rate shows the
  gates cannot fire, the verdict is recorded with the numbers and no flip
  lane starts. Estimated: one measurement lane, corpus-class runs only.
- **Lane 2 (the flip, default-off):** the shared #28 lookup helper, the
  handle table, `rust_subtype_type_id_insert` and `rust_subtype_ids_coded`
  (both with the #42 fetch pattern), the Rust tree store and answer store,
  the residency counters, and the differential suites both gate states. The
  byte path for first-sight and excluded classes is untouched; the smallest
  change that can still evaluate every gate clause, because gate 2 needs
  end-to-end instructions, not a microbench.
- **Lane 3 (the verdict):** the paired battery of section 5, the gate
  arithmetic, the record. Explicitly deferred regardless of verdict: `Arc`
  interning beyond what gate 5 forces, extending TypeId to any other seam,
  the `__weakref__` slots slice (#58 follow-up), any answer cache keyed on
  bytes, and any default flip (a default-on flip would be its own T3 lane
  and only after a PASS).

## 8. Explicit non-goals (from #71)

- **Not a shadow or mirror.** No dual-write capture, no read-flip shadow, no
  per-mutation registration. The failed F1/G4/ADR-0006 shapes are
  structural non-goals, and the design contains none of their machinery.
- **A Rust-side answer cache keyed on received bytes is NOT this
  experiment.** Python still owns the graph and still pays serialization
  before Rust can look it up; that is a call-boundary optimization. It may
  be evaluated separately only if this gate fails, as evidence on
  computation-reuse versus pre-boundary representation work. The TypeId
  answer store in this design is not that cache: its keys are resident
  objects whose serialization has already been paid once and is never
  repeated, which is the ownership claim under test.
- **Instance storage is falsified.** ADR-0006 measured the `Instance`
  replacement view at a 0.2-1.1% funnel ceiling against a 10% bar, +501 MB
  RSS, prototype deleted. This experiment does not reopen it; the family is
  the subtype/type-graph family centered on `rust_is_subtype`, as #71 fixes.
