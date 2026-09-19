# Making a Rust-ported type checker net-faster than its Python host

Report date: 2026-09-19. Mode: FULL (first report for this project).

## Addendum (2026-09-19, same day): scope correction from the reader's reply

The reader's reply overturned this report's existential framing, and the
correction is recorded rather than silently applied: the parity target below
answers "should a Rust kernel embedded in Python ship inside Python mypy?",
which is a question about the **hybrid** architecture. It does not decide
whether the standalone Rust port continues. The project's objective is to make
mypy a Rust program; the standalone architecture is evaluated once
Python-owned representations and the boundary have been eliminated, as
`I(Rust standalone) vs I(upstream mypy)`. Consequences, now canonical in
`docs/remaining-migration-plan.md` and epic #72: correctness gates stay strict,
but the performance gate moves to milestones where old work is actually
deleted; intermediate states that keep both representations alive legitimately
regress, which re-reads the Phase F / G4 / ADR-0006 rejections as measurements
of the hybrid, not of ownership itself. The measurement work below
(attribution, falsifiers, microbenchmarks) stands unchanged.

## 1. Problem

`mypy` is a static type checker for Python: it reads Python source, builds
types for every name, and reports type errors. A migration project (repo
`mypy-rs`, a fork of the upstream `mypy` tree) is porting mypy's type-checking
core, called the **type kernel**, from Python to Rust. The Rust code compiles
to a shared library and is called from the still-Python program across a PyO3
(Rust-to-CPython) boundary. No Python is deleted: every ported piece keeps its
Python fallback, so the program runs both implementations and takes the Rust
answer when Rust returns one. This is a **strangler-fig** port.

The **target claim** under test is exact. On the **cold self-check** (the
checker run over its own source plus mypyc, single process, no incremental
cache), turning the type kernel **on** must retire no more CPU instructions than
turning it **off**. "Kernel-on" is the production default, with every graduated
Rust piece armed; "kernel-off" is the same binary with the type kernel disabled
and the pure-Python fallback running. When kernel-on retires more instructions,
the Rust port is a net performance **loss**. The measured difference is the
**gap**. Closing it to zero is **parity**; going below zero is the goal.

If parity cannot be reached, the migration direction is unproven regardless of
how much code has moved.

## 2. Status in three lines

- **Settled:** roughly 97% of the type-checking call volume already executes in
  Rust (2.43M calls into Rust per self-check, 0.19% falling back to Python), and
  the Rust bytes now outnumber the production Python bytes.
- **Gating obstacle:** the port is nevertheless **+82.4e9 instructions
  (+23.6%) slower** than pure Python. The cost is not in the algorithms but **at
  the call boundary**: about 392 instructions per marshaled argument across
  ~2.4-2.9M calls, plus decode/re-encode and Python rebuild on each side.
- **Asked of the reader:** whether the only un-falsified route to parity (moving
  data *ownership* into Rust so a family's wire path can be deleted) can be made
  net-positive incrementally, or whether "Rust kernel, Python host" is the
  terminal state and cheaper per-call engineering is the right bet instead.

## 3. Definitions and notation

- **Instruction retired.** CPU instructions executed, read from macOS
  `/usr/bin/time -l`. Load-invariant: three runs at host load 35-52 agreed to
  0.5%. Wall-clock seconds are load-contaminated here and never used for a
  conclusion.
- **Seam.** One Rust function exposed to Python across the boundary, e.g.
  `rust_is_subtype`. A **seam call** is one invocation. **Wire** means the byte
  serialization of a Python object graph into the buffer passed across the
  boundary; **wire prep** is the Python work that builds those bytes; **decode**
  is Rust parsing them back.
- **Gate / defer.** Each seam has a gate flag; when off, or when Rust returns
  the sentinel `None`, Python runs its own implementation instead. That fallback
  is a **defer**. Defer rate here is 0.19%.
- **Cold self-check.** `mypy_self_check.ini -n0 --no-incremental -p mypy -p
  mypyc`, single process, 378 files, ending "Success: no issues found".
- **Ownership move.** Any change where Rust keeps state (a decoded object, a
  result, a snapshot) across calls so a later call need not re-serialize or
  re-decode. Contrast a **call-boundary optimization**, which keeps the per-call
  wire but makes that wire cheaper. The phase plan names the ownership move
  Phases F (type graph), G (AST nodes), H (traversal drivers).

## 4. Results established

**The gap.** Three interleaved paired rounds, kernel-on vs kernel-off, on head
`8afb1d4ac` (2026-09-19):

| round | kernel-on | kernel-off | paired delta |
| --- | --- | --- | --- |
| 1 | 430.922e9 | 348.833e9 | +82.088e9 |
| 2 | 431.179e9 | 348.803e9 | +82.376e9 |
| 3 | 430.455e9 | 347.977e9 | +82.478e9 |

Median **+82.4e9 (+23.6%)**, spread 0.5%. Cycles 199.5e9 vs 144.7e9, IPC 2.41
to 2.16: the added cost is memory-bound, not compute-bound. Peak RSS 706 MB vs
585 MB (**+116 MiB**). On an earlier four-arm head (upstream #1878) the same
experiment read Python 366.3e9, kernel-on 483.7e9, i.e. **+117.4e9 (+32%)**.
Verified in `docs/plans/2026-09-19-gap-attribution.md` (#61).

**Where the gap lives** (six buckets; magnitudes from load-independent sources
only):

| bucket | instructions | share |
| --- | ---: | ---: |
| Rust kernel self (wire decode, solve, re-encode) | +27e9 | 33% |
| Python serialize prep (per-seam args, dedup keys, snapshot bytes, cache upkeep) | +20e9 | 24% |
| Allocator/GC amplification (from +116 MiB working set) | +7e9 | 9% |
| Deserialize/rebuild back to Python objects | +5e9 | 6% |
| Seam crossings + gate glue (2.43M calls) | +5e9 | 6% |
| Residual (model error) | +18e9 | 22% |

**The boundary is the cost, not the algorithm.** Instruction-denominated
microbenchmarks, each against a same-shape control loop:

| operation | instructions |
| --- | ---: |
| bare 1-argument PyO3 crossing | 102.1 |
| 12-argument coded call | 4,410.9 |
| Rust to Python callback | 3,960.5 (vs 65.1 for a direct Python call) |
| one serialize miss, `Instance` node | 16,116.4 |

Marshaling therefore runs about **392 instructions per argument**, roughly
**40x** the crossing itself, and a Rust driver calling back into Python pays
about **60x** the Python body it would replace.

**The wire-prep ledger** (clocks around 845 patched seams): 3.53s of
seam-attributed prep plus 0.24s unconsumed, plus **1.42s of adapter residual**
no ledger sees (raw `t.write` buffer prep, gate checks, argument glue). Prep on
*deferred* calls is **0.007s**, so the "skip prep before a call that will defer"
lever is dead.

**Code volume moved.** Rust `crates/`: **183,501** lines (type_kernel 171,482;
ast_serialize 8,887; module_resolver 3,092; fs_probe 40). Production Python
(`mypy/`, excluding tests and bundled typeshed stubs): **144,316** lines, the
largest files being `checker.py` (12,875), `semanal.py` (10,371) and
`checkexpr.py` (9,484). `nodes.py` and `types.py` are plugin-visible mutable
object graphs (the Phase E1 constraint) and stay Python until a Rust-owned
redesign lands. Seam inventory: **830 registered, 305 called, 532 never called**.

**Claim ladder.** "Rust kernel, Python host" is today's rung. Phase F
(Rust-owned type graph) closed **unclaimed** (#1573): every shipped mechanism
measured zero or worse. Phase G4 (Rust-owned AST read surface, per family) was
claimed and **withdrawn the same day** (#1867/#1869/#1870): each per-family gate
measured only the *marginal* cost with capture on in both arms, while the
capture itself costs **+23% instructions** over Python, so the whole read
surface is parked behind a default-off flag. Phase H is not started. (Evidence:
`docs/remaining-migration-plan.md`, claim-ladder section.)

## 5. Approaches tried and failed

Each item was a plausible route to the gap, carried to a **pre-registered gate**
fixed before measurement, and broke with a named number.

1. **Incremental resolver snapshot (lever 1).** Hypothesis: the resolver
   re-serializes its whole symbol graph on every incremental collect, and the
   attribution counter `re_pushed_builtins: 56,138` was that volume. Gate: if
   under 50% of re-serialized entries are unchanged between collects, drop.
   **Verdict NO-GO** (`docs/plans/2026-09-19-resolver-snap-falsifier.md`, #66).
   The premise was wrong: the counter counted re-**collected** entries, not
   re-**pushed** ones. The actual feed to Rust over a whole cold self-check is
   **1 entry** (`builtins.int`, caught by the existing signature gate); 61,626
   entries are walked again, 99.995% unchanged, all already skipped. `write_str`
   inside the resolver build is 1.6% of calls. Addressable saving: ~3 KB per run
   against a 3-5e9 ceiling.

2. **Kernel wire-structure churn (lever 2).** Hypothesis: the kernel's 33% self
   share is dominated by decode and clone of `wire::Type`; interning decoded
   trees or a borrowed decode halves it. Gate: if decode+clone+drop+hash is
   under 30% of kernel self, drop. **Verdict NO-GO**
   (`docs/plans/2026-09-19-wire-churn-falsifier.md`, #63). Sample route: 23.2%
   median, 28.1% charging every adjacent family, both under the gate. **~97% of
   clone frames are solve-side** (meet, checkmember, expandtype, freshen,
   constraints); only ~3.4% sit under the wire read/write path. The
   decodable-and-clonable surface is ~6% of kernel self: the churn is a property
   of the solve algorithms building temporary trees, not of the wire.

3. **Identity memo for subtype answers.** Hypothesis: cache `is_subtype` results
   keyed on a weak identity of the operands. Measured 41,711 of 127,096 lookups
   (32.8%) are identity repeats, clearing the nominal gate, but **NO-GO on
   soundness** (#58): `Type` uses `__slots__` without `__weakref__`, so a weak
   reference is impossible (149,710 of 149,710 entries reject `weakref.ref`), and
   a raw-id key is unsound (84 recycled ids in one build). Verifying the memo by
   comparing bytes would re-serialize, the cost the memo exists to avoid.

4. **Typeview / one-family replacement view (Phase F reopening).** Hypothesis:
   move `Instance` field storage into Rust so the wire seam gets cheaper. Gate:
   the prototype must beat the native default by at least 10% relative total work
   share. **Verdict Rejected** (ADR-0006, #54), prototype deleted. The funnel
   addressable is **0.2-1.1%** of total work (byte share 1.08%), 10-50x below the
   bar; 0 defers; RSS +501 MB.

5. **`rust_expand_type` / `rust_check_overload_call` retirement.** A seam-level
   defer A/B measured both as net losses end-to-end (`rust_expand_type` -1.66%),
   retired, summing to only ~10% of the gap. Lesson on record: per-shape
   microbenchmarks are not end-to-end evidence (`rust_expand_type` benchmarked
   "2.3x faster" yet lost), and `rust_analyze_instance_member_dispatch`, profiled
   at 20.5 us/call and suspected a loss, is a net **win** (+0.88%). Pre-gating
   deferred calls (a sixth candidate) is dead: 0.007s of prep sits on them.

**The pattern:** every lever measurable cheaply dissolved at its gate. Two of the
top three failed on a counter-name misread, a measurement-layer defect rather
than an engineering one.

## 6. Open conjectures

1. **Call-boundary optimization cannot reach parity.** The surviving levers
   (deserialize-back avoidance; `_is_subtype` dedup-key serialization;
   argument-marshal reduction) have ceilings summing to **~12-24e9 (15-29%)**,
   under the 82e9 needed. Confidence high. Cheapest next test: bench a
   prebuilt-blob variant of a 12-argument coded seam.
2. **Deserialize-back avoidance (lever 3).** 89k full rebuilds, 206k fixup
   walks, 468k `copy.copy`. Ceiling 2-4e9. Pre-registered falsifier: count
   rebuilds whose input bytes repeat within a build; under 30% repeat, drop.
   Untested.
3. **`_is_subtype` dedup-key serialization (lever 4).** 0.47s consumed plus
   0.24s unconsumed per build, 85% repeat lookups, key bytes rebuilt every time.
   Ceiling 2-4e9, feasibility constrained by #58 (no weak identity). Sound paths
   left: a `__weakref__`-slots slice with its own RSS+instruction A/B, or serving
   the answer cache Rust-side keyed on bytes Rust already holds. Untested.
4. **The ownership move is the only route to parity, and it is net-negative when
   landed incrementally.** Confidence medium. Falsified if a bounded ownership
   step that *deletes* a family's wire path measures net-clear end-to-end.
   Cheapest next test: let Rust hold one family's decoded tree across calls and
   measure the full cold self-check with that family's wire path removed.
   Evidence against: every incremental ownership step tried (F shadow storage,
   G4 node mirror) added capture cost on top of a live wire path.

## 7. Related academic work

Citations verified against Crossref DOI metadata, and PDF full text where noted,
on 2026-09-19. Each entry states what it proves and whether it transfers.

**Grimmer, Schatz, Seaton, Würthinger, Luján, Mössenböck. "Cross-Language
Interoperability in a Multi-Language Runtime." ACM TOPLAS 40(2), Article 8,
2018. DOI 10.1145/3201898** (companion: DLS '15, DOI 10.1145/2816707.2816714).
Proves that composing languages in one VM (TruffleVM) works when the boundary
passes language-agnostic *generic access* messages the JIT specializes.
Technique: all languages share one AST-level IR and one optimizing compiler, so a
cross-boundary call can be inlined and optimized across. It **transfers
conceptually, not mechanically**: CPython and Rust share no JIT, so this boundary
cannot be fused. The transferable result is the counterfactual, that boundary
cost is dominated by marshaling unless the crossing is made coarse and cacheable,
which is what this project's plain-record, bytes-and-IDs seams attempt.

**Grimmer, Rigger, Stadler, Schatz, Mössenböck. "An Efficient Native Function
Interface for Java." PPPJ '13, pp. 35-44. DOI 10.1145/2500828.2500832.** Proves
by measurement that JNI per-call overhead is real and avoidable: a
compiler-integrated native interface beats JNI and JNA in every relevant case, by
up to 1.9x. Technique: generated call stubs letting compiled code call native
code directly, with compiler-specializable argument marshaling. It **transfers**:
this is the cleanest measured decomposition of a managed-to-native boundary into
dispatch, marshaling and lookup, the same categories as a PyO3 boundary. A Rust
extension removes the dispatch stub (the extension *is* native), leaving
marshaling as the term to engineer down: direct evidence for batching behind a
narrow interface rather than porting per-node calls.

**Yallop, Sheets, Madhavapeddy. "A Modular Foreign Function Interface." Science
of Computer Programming 164, pp. 82-97, 2018. DOI 10.1016/j.scico.2017.04.002.**
Proves a declarative foreign-function specification can be separated from its
binding mechanism, so one specification runs under static or dynamic,
synchronous or asynchronous mechanisms unchanged. It **transfers at the design
axis, not the cost axis**: the binding mechanism is independently choosable,
which is what keeping every Python fallback swappable behind an
`Options.native_*` gate already does.

**Mokhov, Mitchell, Peyton Jones. "Build systems à la carte." PACMPL 2(ICFP),
2018. DOI 10.1145/3236774** (journal version: JFP 30, e8, 2020, DOI
10.1017/S0956796820000088). Proves build systems are points in one design space
defined by orthogonal components, a scheduler and a rebuilder, with the key
rebuilder split between verifying-trace (dirty-bit, like Make) and
constructive-trace (like Bazel) strategies. It **transfers directly to the
caching question**: mypy's incremental mode is a verifying-trace build over
module dependency records, and moving that graph into the Rust resolver changes
the implementation, not the abstraction. Rebuild correctness is a property of
trace/rebuild semantics, not implementation language, which is why the parity
suites, not the port, hold incremental-mode correctness.

**Barany. "Python Interpreter Performance Deconstructed." DYLA '14, pp. 1-9. DOI
10.1145/2617548.2617552.** Proves by per-feature ablation that boxed numbers cost
up to 43% of numeric-benchmark time and late binding of calls up to 30% on
object-oriented code, while reference counting and dynamic type checks cost
comparatively less. Technique: a compiler emitting code inside CPython with
individually toggleable optimizations, so runtime deltas give per-feature costs.
It **transfers with a caveat**: it is a 2014 workshop paper about
interpreter-internal costs, not a measurement of C-API extension-call overhead as
such. The literature search found **no peer-reviewed paper that measures CPython
C-API boundary crossing as its object of study**; Barany is the closest credible
indexed evidence, and the costs it finds are paid at the marshaling layer, not
inside the Rust code.

**Not academic, provenance only:** "strangler fig" comes from Martin Fowler's
2004 note of that name (martinfowler.com).

## 8. Constraints

- Target: parity or better on the cold self-check, in retired instructions,
  single process. Not throughput, not latency, not a microbenchmark.
- The Python graph stays canonical during the port; fallbacks cannot be deleted
  until a phase graduates, so the denominator never shrinks.
- Rust and Python must emit byte-identical diagnostics; every change runs a
  differential parity suite with and without the gate.
- The boundary is PyO3 (CPython C-API), not a custom ABI; no batched or zero-copy
  transport is in place.
- `Type` and Node objects are `__slots__`-based and plugin-visible: no
  `__weakref__`, no ownership move without a multi-quarter redesign.
- Heavy jobs share a 51 GB combined-RSS pool; one corpus job at a time.

## 9. Questions

1. Is "Rust kernel, Python host" a legitimate terminal state, or should the
   project commit a bounded ownership step that deletes one family's wire path
   outright, accepting a temporary regression, rather than measuring incremental
   steps that each carry two live paths? *A useful answer names the family and
   the acceptance test.*
2. Given the measured per-argument marshaling cost (~392 instructions/arg, ~40x
   the crossing), is blob or single-buffer transport the highest-value unexplored
   engineering move, or is argument count structurally irreducible? *A useful
   answer says which seams could batch.*
3. Should the identity-memo impossibility (#58) be worked around on the Rust side
   (answer cache keyed on received bytes) rather than the Python side, and is
   that a call-boundary or an ownership move? *A useful answer decides which.*
4. If per-call levers cap at ~15-29% and parity needs 100%, what is the next
   falsifiable experiment that could kill the ownership route early? *A useful
   answer is a gate with a pre-registered threshold.*

## 10. Appendix pointers

- Gap attribution: `docs/plans/2026-09-19-gap-attribution.md` (#61).
- Lever 1 falsifier: `docs/plans/2026-09-19-resolver-snap-falsifier.md` (#66).
- Lever 2 falsifier: `docs/plans/2026-09-19-wire-churn-falsifier.md` (#63).
- Identity-memo NO-GO: `docs/plans/2026-09-19-perf-subtype-identity-probe.md`
  (#58, #59).
- Typeview rejection: `docs/adrs/0006-instance-replacement-view.md` (#54).
- Migration plan and claim ladder: `docs/remaining-migration-plan.md`.
- Seam inventory: `docs/plans/type-kernel-seam-ledger.md`.
- Raw evidence: `/private/tmp/gap-attrib/`, `/private/tmp/wire-churn/`,
  `/private/tmp/resolver-snap/`.
- Upstream parity issues: #1624 (existential blocker), #1878 (per-call overhead).
