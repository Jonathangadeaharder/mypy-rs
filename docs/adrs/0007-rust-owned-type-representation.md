# ADR-0007: Rust-owned Type representation (ownership inversion)

- Status: Proposed (design record for epic #72 roadmap step 1; no production
  change in this lane). The bounded first experiment this decision enables is
  `docs/plans/2026-09-19-type-owner-inversion-brief.md`, in the same PR.
- Date: 2026-09-19
- Issue: #72 (step 1: "Rust owns `Type`")
- Supersedes: the "shadow, never replacement" premise of ADR-0004 Decision 1
  (`docs/adrs/0004-e1-stage-b-type-proxy-reflect-model.md:75-123`) and the
  ADR-0006 rejection grounds, both of which were decided under the old
  net-positive-now performance rule that epic #72 has corrected.
- Does NOT supersede: ADR-0001 Decision 1
  (`docs/adrs/0001-phase-e1-visit-porting-decisions.md:29-73`) for
  `TypeInfo`/`nodes.py` storage — this ADR inverts the `mypy/types.py` Type
  graph only; `TypeInfo` and AST-node ownership is epic step 2.
- Follows: ADR-0003 (wire Type totality; the wire codec remains the
  cross-representation lingua franca during the migration).

## Context

The hybrid instruction gap is +82.478e9 instructions (kernel-on 430.5e9 vs
kernel-off 348.0e9, cold self-check; `docs/plans/2026-09-19-gap-attribution.md:37`).
Every boundary lever that could have closed it from inside the hybrid has now
been measured dead or capped far below it:

- Lever 1 (resolver-snapshot re-serialization): NO-GO, #66
  (`docs/plans/2026-09-19-resolver-snap-falsifier.md`).
- Lever 2 (wire churn): NO-GO, #63
  (`docs/plans/2026-09-19-wire-churn-falsifier.md`).
- Pre-gating deferred calls: dead, 0.007 s of addressable work
  (attribution, "Wire-prep ledger").
- Identity-keyed front memo: structurally impossible, #58 — slots-based
  `Type` objects have no `__weakref__` (149,710/149,710 rejections) and a
  raw-id key is unsound (84 recycled ids in one build)
  (`docs/plans/2026-09-19-perf-subtype-identity-probe.md:44-52`, `:104`).
- Blob arg-packing on coded seams: NO-GO, #70 — a prebuilt buffer cuts the
  12-arg coded call only 33.9% against a >=70% gate, the deployable
  build-included form is net negative (-27.9%), the global ceiling is
  0.98e9 < 1e9, and call batching is structurally blocked because answers
  are consumed synchronously per question
  (`docs/plans/2026-09-19-blob-marshal-falsifier.md:84-89`, `:131`,
  `:155-187`). The same record corrected the marshal attribution to ~139
  instr/arg (`:110`), which lowers every remaining call-path ceiling.
- Residency (stable TypeIds over a Python-canonical graph): NO-GO on
  measurement before any production code, #71 Step 0 — repeat-class
  populations cannot fire either gate-1 reading (84.2%/75.2% and a 61.1%
  total-wire ceiling vs 95%), gate 2 has a hard population-bounded ceiling
  of 0.43 < 0.5, and 38.9% of operand appearances are first-sight objects
  (`docs/plans/2026-09-19-subtype-residency-step0.md:124`, `:143`,
  `:148-181`; PR #78).

Two earlier architectures for moving type storage were rejected on
measurement, not contract: the ADR-0004 proxy shadow (measured ceiling
<=0.5% wall, P2/P2b) and the ADR-0006 one-family replacement view
(funnel-scoped ceiling 0.2-1.1% against a 10% bar, +501 MB RSS, prototype
deleted; `docs/adrs/0006-instance-replacement-view.md:317-346`, `:384-397`).
Both kept Python canonical and were judged by "is the hybrid net-positive
today". Epic #72 corrects the rule: correctness gates stay strict, but
performance parity is evaluated at architectural milestones where old work is
actually deleted, not at intermediate dual-representation states
(#72 roadmap; `docs/remaining-migration-plan.md:18-27`). ADR-0006's own
closing finding — that all four ADR-0004 contract objections (plugins,
`isinstance`, `__slots__`, astmerge) measured *satisfiable*
(`docs/adrs/0006-instance-replacement-view.md:384-398`) — is the measured
fact that makes the contract side of this ADR tractable.

Step 0 of #71 also located where the mass actually is: 53% of content hits
have at least one fresh operand, largely self-manufactured deser-back churn
(attribution lever 3; step0 record, "Verdict"). Under a Python-canonical
graph, serialize-at-most-once cannot delete a first walk for a genuinely new
object. Under the inversion there is no serialize step at all for flipped
families — the kernel reads arena records, and results are minted as facades,
not decoded from bytes — so the deser-back churn class is deleted with the
byte path rather than paid down. That is the qualitative difference the #71
on-failure clause asks for, and this ADR is the record of it.

## Decision

**Rust owns the canonical type representation.** A per-build, per-thread Rust
arena maps `TypeId -> TypeRecord`, where each record is the Rust-native form
of one `mypy.types.Type` object (the `wire::Type` enum shape,
`crates/type_kernel/src/wire.rs:666-806`, extended to reference children by
`TypeId` instead of inline boxes). Python-facing `Type` objects stop being
canonical: they become **facades** — same classes, same names, same
`isinstance` behavior — whose instance state is a `TypeId` (plus
position/line data owned by `mypy.nodes.Context`, `mypy/nodes.py:157-161`)
and whose type-data attribute access routes to the arena. Authority moves
one direction: the arena record is the truth, the facade is an access
surface, and per-family flips delete the Python-side field storage rather
than adding a second copy of it.

The inversion proceeds family by family, behind a default-off gate, with the
wire path (ADR-0003 codec, `_serialize_type_for_visitor`,
`mypy/types.py:4746`) retained for every un-flipped family and for cache
serialization throughout. The first bounded slice and its pre-registered
gates are the companion brief; this ADR fixes the architecture and the
contract answers.

### Decision 1: TypeId protocol and arena lifetime

- `TypeId` extends the existing stable-handle service
  (`crates/type_kernel/src/identity.rs:171`, `handle_for_stable`): one handle
  namespace, no second identity mint. The guarantees are inherited:
  thread-local (`identity.rs:10-12`), idempotent per live object
  (`:13-15`), pinned strongly so a recycled `id()` can never adopt a stale
  handle (`:23-32`).
- The arena is scoped like the identity registry: thread-local, so parallel
  build workers (`MYPY_NUM_WORKERS > 0`) and daemon processes each own an
  independent arena. A `TypeId` is meaningful only inside the process and
  thread that minted it (identity guarantee 1); nothing serializes it.
- The arena clears at exactly the boundaries that clear per-build native
  state today (`_clear_native_resolvers` callers, e.g.
  `mypy/server/update.py:1128-1130`), with the same preserving-reset split
  the identity service already defines for daemon rechecks
  (`identity.rs:33-42`, `reset(preserve_stable)`, `sweep_stable` at `:228`):
  a recheck preserves arena entries whose facade objects astmerge preserved,
  and releases entries whose only owner was the pin.
- Minting happens at facade construction: `Type.__init__`-family
  constructors allocate the arena record and store the returned `TypeId`.
  This is the only mint path, which is what makes the facade's lack of field
  storage sound: there is no moment where data exists in Python and must be
  copied into Rust.

### Decision 2: the facade contract (plugin compatibility)

What a facade must support, concretely, and how:

- **`isinstance`**: unchanged by construction. The facade *is* the existing
  class (`AnyType`, `Instance`, ... in `mypy/types.py`); the class hierarchy,
  `Type`/`ProperType`/`FunctionLike` split, and subclass checks are exactly
  today's. `isinstance(t, Instance)` and every `visit_*` dispatch on class
  (`Type.accept`, `mypy/types.py:519-520`) keep working.
- **Attribute reads**: type-data attributes (e.g. `Instance.args`,
  `AnyType.type_of_any`) are served from the arena record via class-level
  descriptors. Position attributes (`line`/`column`/`end_line`/`end_column`)
  stay Python-side slots on `mypy.nodes.Context` (`mypy/nodes.py:161`) —
  they are node facts, mutated in place by `set_line`, and are not type
  data. CPython-visible reads of *scalar* immutable record fields may be
  served from a facade-side cache coherent by construction (no write path
  exists for the first slice's families); per-family caching decisions are
  made by measurement in the family's lane, not by default here. The
  measured counter-evidence stands: ADR-0006's arm-2 routed 17.43M `args`
  reads through Rust storage and was strictly negative
  (`docs/adrs/0006-instance-replacement-view.md:249-256`), so read routing is
  treated as the make-or-break cost question of the hybrid window and is
  pre-measured per family before any flip (brief section 5).
- **Attribute mutation**: for the first slice's families there are no
  checker-phase in-place mutators (ADR-0004 Decision 2 inventory: `AnyType`
  fields O, `UninhabitedType.ambiguous` O, `NoneType`/`ErasedType` unit,
  `DeletedType.source` O;
  `docs/adrs/0004-e1-stage-b-type-proxy-reflect-model.md:182-193`),
  so mutation is
  not part of slice 1. For later, mutable families the write path is
  write-through: the facade's `__setattr__` updates the arena record
  atomically and the Python side holds nothing to go stale. This is the
  structural difference from ADR-0006's capture (see Decision 4) — and the
  reason the mutable families are explicitly out of the first slice.
- **`__eq__` / hashing**: structural, against arena records, preserving
  today's per-class semantics (e.g. `Instance.__eq__` compares
  type/args/last_known_value/extra_attrs, `mypy/types.py:2020-2028`;
  `__hash__` memoizes, `:2015-2018`). The memo slot moves into the arena
  record. Equal facades across construction sites remain equal because
  equality is field-based today, not identity-based; the inversion changes
  nothing here.
- **Traversal**: `accept(visitor)` stays; visitors receive the facade and
  their attribute reads route to the arena. `TypeStrVisitor`,
  `HasTypeVars`, and every `TypeVisitor` in `mypy/types.py` keep compiling
  against unchanged class signatures.
- **`__slots__`-based subclasses**: facades keep `__slots__`
  (`Type.__slots__`, `mypy/types.py:472`). The slot *contents* change for
  flipped families (data fields removed, handle fields added) — a visible
  layout change to any third-party code that subclasses `mypy.types` classes
  and assigns its own fields. No in-tree plugin or mypy code subclasses
  `Type` variants; the break is accepted, documented, and confined to the
  gate-on state.
- **Pickling**: no mypy consumer pickles `Type` objects — the only pickle
  in the daemon surface is options data (`mypy/dmypy_server.py:87`). Since a
  pickled facade would smuggle a process-local `TypeId` across processes,
  facades define `__reduce__` to raise, fail-early, rather than silently
  producing a corrupt object in another process. If a consumer ever
  materializes, pickle support is a dedicated slice (see/serialize remain
  available), not an afterthought.

### Decision 3: astmerge, daemon, incremental and cache semantics

- **On-disk cache**: a `TypeId` never touches it. The incremental cache
  stores self-describing serialized types (JSON `serialize()`/
  `deserialize()`, e.g. `Instance.serialize` `mypy/types.py:2030-2036`, and
  the binary wire stream inside `mypy.cache`), which contain fullnames and
  values, never handles. The inversion changes no serialized byte; the
  gate is excluded from `OPTIONS_AFFECTING_CACHE` and `CACHE_VERSION` is
  untouched until a graduation deletes the byte path for a family (its own
  T3 lane). Cache deserialization constructs facades through the same
  constructor path, which mints fresh, process-local `TypeId`s.
- **Cross-process**: parallel workers pass module trees and cache blobs
  between processes, never live `Type` objects. Because the arena is
  thread-local and the handle registry is thread-local (identity guarantee
  1), no process can observe another's `TypeId`. There is no cross-process
  identity contract to preserve that does not already run through the
  cache today.
- **Daemon / fine-grained updates**: `merge_asts` preserves the identities
  of externally visible nodes and re-homes the rest
  (`mypy/server/astmerge.py:30`, `:124`, `:187-188`); the update path then
  rebuilds native resolver state at the recheck boundary
  (`mypy/server/update.py:1124-1131`). Under the inversion an
  astmerge-preserved facade keeps its `TypeId` (preserving reset,
  identity.rs:33-42); a re-created node gets a fresh facade and a fresh
  `TypeId`, which is exactly today's identity semantics for re-created
  nodes. The arena reset rides the existing `_clear_native_resolvers`
  boundary chain; no new invalidation mechanism is invented.
- **Incremental mode**: unchanged contract — a cache hit reconstructs the
  same *values* as a fresh build. Values are structural (Decision 2
  equality); `TypeId`s are internal and never compared across builds.

### Decision 4: why this does not reproduce ADR-0004/0006

ADR-0006's prototype kept the Python object canonical, added Rust field
storage as a *derived* view, and paid for the derivation on every mutation:
3.21M registrations through an installed `__setattr__` interceptor
(`docs/adrs/0006-instance-replacement-view.md:80-97`, `:262-278`), +501 MB
to +1.67 GB RSS for pins (`:337-338`, `:226`), and an addressable surface
of at most the serialize funnel, 0.2-2.1% of total work (`:225`). Its
capture could only preempt encodes the F3 wire cache would mostly have
served anyway (served byte share 1.08%, `:317-324`).

The inversion removes each failure precondition:

1. **Authority**: the arena record is the truth. There is no derivation to
   keep in step, because the Python copy does not exist — the facade holds
   a handle, not fields. Dual representation exists only per un-flipped
   family during the migration window, and each family's flip deletes its
   Python fields (the deletion-framed progress gate, brief section 4).
2. **Capture**: for the first slice's immutable families there is no
   mutation to capture — mint-at-construction is the only write, at a site
   that already existed (the constructor). For later mutable families the
   facade's write path *is* the arena write (write-through, one store),
   not an interceptor duplicating a store the Python object also performs.
   The ADR-0006 tax was paying twice per mutation; here there is one store.
3. **Funnel ceiling**: ADR-0006 was confined to the funnel because the
   Python graph stayed authoritative, so all it could change was which
   bytes a walk produced. The inversion is not funnel-scoped: the kernel's
   long-term direction is to consume `TypeId`s (epic steps 3-4 collapse
   seams inward), and results mint facades without byte decode. Judged by
   the corrected milestone rule, the intermediate state is not required to
   be net-positive — it is required to be correct and to delete.

### Decision 5: identity and weakness when Rust owns identity

The #58 impossibility — weak identity keying on slots-based `Type` objects —
dissolves rather than being worked around. #58 needed weakrefs because the
memo had to observe object death from *outside* the object's lifecycle while
Python stayed canonical and the memo was optional. Under the inversion:

- Identity is not `id()`. A `TypeId` is pinned to its facade by the
  identity service's stable layer (`identity.rs:23-32`), so a recycled
  `id()` can never alias a live handle; the 84-recycled-id hazard
  (`docs/plans/2026-09-19-perf-subtype-identity-probe.md:104`) is
  structurally unreachable, not detected-and-dodged.
- Liveness is the arena's own bookkeeping, and it uses the shipped
  protocol: strong pins plus boundary sweeps (`sweep_stable`,
  `identity.rs:228-239`). The documented residual — an object retained by
  a reference cycle keeps its pin until the next non-preserving reset
  (`identity.rs:33-42`) — is accepted unchanged, and it is the same
  residual the F3 wire cache already documents
  (`mypy/types.py:133-136`).
- **What replaces weakref semantics: nothing, because no consumer needs
  them.** There is no weakref identity map over `Type` objects anywhere in
  `mypy/` today (ADR-0004 Decision 7's audit; only the gc-based debug
  blacklist in `mypy/server/objgraph.py` exists). If a future consumer
  needs to observe facade death, the facade class definitions are now
  ours to change: adding `("__weakref__",)` to a facade's slots is a
  deliberate layout decision at that point, with its own A/B — the #58
  follow-up (a) — rather than an impossibility.

### Decision 6: mixed graphs during the migration

Until every family is flipped, graphs mix flipped and un-flipped nodes (a
`UnionType` facade holding an `AnyType` child and an `Instance` child).
The wire codec (ADR-0003 totality) remains the lingua franca:

- The arena record for a flipped node stores un-flipped children as inline
  wire blobs minted through the existing funnel at construction time,
  under the shipped F3 acceptance predicate (phase gate, pre-fixup
  exclusion, tvar fingerprint; `mypy/types.py:133-152`, `:206-225`) —
  the same refusal rules the F3 cache proves safe, so a record that
  cannot be minted soundly is not minted and the family falls back to
  the byte path. A miss is the status quo, never a stale byte.
- A flipped child of an un-flipped parent serializes from its arena record
  (encode-at-mint, coherent by immutability for slice 1), so the Python
  walk for that child is unreachable — the deletion the brief counts.
- Seams keep their current signatures in slice 1 (bytes in, i8/results
  out); `TypeId`-taking seam variants belong to the families whose primary
  operand traffic is theirs (the next slices), not to this decision.

### Decision 7: rejected alternatives

1. **Stay hybrid, optimize the boundary.** Measured dead: every lever
   listed in Context; the gap stands at +82.4e9 with no unmeasured lever
   remaining (blob falsifier, "Verdict", `:189-200`).
2. **ADR-0004 proxy shadow (Python canonical, reflect-on-write).**
   Measured ceiling <=0.5% wall (P2/P2b); superseded by ADR-0006; its
   O/R/S inventory is inherited by this ADR as the per-family mutability
   audit.
3. **ADR-0006 replacement view (Python canonical, field storage moved).**
   Rejected on measurement: 0.2-1.1% funnel ceiling, +501 MB RSS, capture
   and routed-read halves both measured negative. Its contract findings
   (all four surfaces satisfiable) are inherited as evidence for
   Decision 2.
4. **#71 residency (stable TypeIds over a Python-canonical graph, no
   authority move).** NO-GO on measurement: the repeat population is too
   small to carry the gates (ceiling 0.43 < 0.5) and first-sight objects —
   38.9% of appearances — cannot have their first walk deleted while
   Python is canonical. The inversion is the qualitatively different
   architecture the #71 on-failure clause names: it deletes the walk
   itself, first sight included, and removes the deser-back churn class
   with the byte path.
5. **Bytes-keyed answer cache.** Excluded by #71's non-goals and this
   epic: Python still pays serialization before Rust can look anything
   up (`docs/plans/2026-09-19-subtype-residency-brief.md:540-547`). Not a
   form of ownership; not reopened here.
6. **Flip all families at once.** Unbounded blast radius: `Instance` and
   `CallableType` carry checker-phase in-place mutators (ADR-0004
   inventory, `:206-283`) and plugin-visible shared mutation; flipping
   them requires the write-through design plus the touch-point audit
   before any code. Rejected in favor of the immutable-family first slice
   (companion brief), which proves the representation mechanics with the
   coherence problem reduced to immutability.
7. **Port the checker traversal first (epic step 3 out of order).**
   Rejected: traversal ownership without representation ownership would
   multiply the wire boundary, not remove it (the measured lesson of
   every seam added since M17; `docs/remaining-migration-plan.md:394-404`).

## Consequences

- Correctness is gated per family by the existing differential discipline
  (testcheck + parity suites both gate states, byte-identical diagnostics);
  performance is gated at deletion milestones per epic #72, with the
  slice's counters proving the Python walk is *unreachable* for served
  constructions, not merely faster.
- `mypy/types.py` field storage shrinks family by family; the wire codec
  survives until the last family graduates, then the serializer leaves the
  checker (epic step 1's end state) and remains for cache I/O.
- The plugin API is unchanged in shape: plugins receive facades through
  the same contexts (`FunctionContext`, `mypy/plugin.py:451`;
  `AttributeContext`, `:505`; `ClassDefContext`, `:514`) and observe the
  same classes, equality, and (for un-flipped families) mutation
  semantics. The one accepted break is the slot-layout change for
  third-party `Type` subclasses under the gate (Decision 2).
- The parts of `mypy/types.py` that cannot invert yet — the mutable,
  plugin-visible families (`Instance.args` under semanal, `CallableType`
  field rewrites under checking, `TypeVarLikeType` meta-level mutation) —
  keep the byte path as their fallback, unchanged, until their
  write-through lanes land. The companion brief states this per family.
- `TypeInfo`/`nodes.py` (epic step 2), semantic analysis (step 4) and the
  remaining roadmap steps are out of scope; nothing in this ADR pre-empts
  their designs.
