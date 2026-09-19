# E1 Stage B proxy design brief: the rung was built, measured, and is dead

Date: 2026-09-19. Lane: design + recon only, no production code touched.
Base: `main` at `4f25c348a`. Commissioned as "produce the E1 Stage B design
brief and a concrete, falsifiable first-rung plan".

## Headline: the task premise is stale, and the evidence says so

The commission states Stage B (proxy model) is "unbuilt". The tree and the
document chain say otherwise. E1 Stage B has been **designed twice, built
twice, and measured dead twice**:

1. **Designed** in ADR-0004 (`docs/adrs/0004-e1-stage-b-type-proxy-reflect-model.md`,
   2026-08-29, upstream #1139): the full reflect-into-Python-on-write shadow
   model, with a mechanical O/R/S field inventory for all 20 wire variants.
2. **Built (shadow arm)**: ADR-0005 P1 scaffold landed (#1553, PR #1558),
   P2 lazy Instance read shadow dropped after A/B measurement
   (wall -0.5%, CPU +5.6% median; P2b thresholded: corpus A +8.97% wall,
   corpus B -11% inside noise, ceiling <=0.5%)
   (`docs/plans/2026-09-11-f-program-close-out.md:23-24`).
3. **Built (replacement-view arm)**: ADR-0006, upstream #1671, landed in
   PR #1694. This is a one-family `Instance` replacement view, in the tree
   today, env-gated `MYPY_TYPE_VIEW` 0/1/2, default off:
   `crates/type_kernel/src/typeview.rs` (871 lines), `mypy/typeview.py`
   (403 lines), `mypy/test/testtypeview.py` (511 lines, 33 test defs),
   funnel probe at `mypy/types.py:4769-4775`, activation at
   `mypy/build.py:1405-1416`, reset at `:2022-2028`, stats at `:2336-2342`.
4. **Measured dead**: ADR-0006's verdict is **NO-GO by ~4.7x, on
   arithmetic** (`docs/adrs/0006-instance-replacement-view.md:297-311`).
   The 2026-09-16 retirements widened the gap to **~9-11x**
   (`docs/plans/2026-09-11-f-program-close-out.md:44-47`,
   `docs/remaining-migration-plan.md:664-676`).

Per the commission's own rule ("if E1 Stage B is actually already partially
built or a documented blocker makes the first rung infeasible, say so
plainly with evidence instead of producing an optimistic brief"), this
brief is that plain statement. It reconciles the record, answers the
proxy-requirements question against the real code, names the falsifiable
criterion the requested rung must clear, shows why no variant of this
family can clear it, and defines the one actionable slice that remains:
the disposition of the already-built prototype.

## 1. Document reconciliation

The chain, in dependency order:

- **ADR-0001** (`docs/adrs/0001-phase-e1-visit-porting-decisions.md:29-37`):
  Decision 1 rejects a Rust arena of `TypeInfo`. Any Rust-owned `Type`
  holds `TypeInfo`/`TypeAlias` by identity (fullname key), never by value.
  This is inherited, not re-litigated: ADR-0004 rule 3 and ADR-0006's
  store shape both obey it (`typeview.rs` keys entries by
  `identity::handle_for_stable` and stores `Instance.type` as a fullname).
- **ADR-0002** (`docs/adrs/0002-remaining-migration-architecture.md:103-123`):
  Decision 4 makes E1 "a design line, not a production flip": Stage A
  (wire totality) then Stage B (proxy), both behind `native_type_kernel`,
  no fallback removal until the wire Type enum round-trips every variant
  and gate-off/on parity is byte-identical for 100% of seams.
- **ADR-0003** (Stage A): done. The ErasedType decode contract is the
  scoped opt-in `_ALLOW_WIRE_ERASED_TYPE` (`mypy/types.py:5532`, refuse
  branch `:5576`), deliberately not global. The Python decode gate binds
  `read_type`, not the Rust enum: a Rust-side shadow may hold
  `Type::ErasedType` freely (`crates/type_kernel/src/wire.rs:103`,
  tag 122).
- **ADR-0004** (Stage B shadow design) and **ADR-0005** (correction of
  its storage shape; P1 landed, P2-P4 pending) then **ADR-0006**
  (replacement-view experiment, succeeds ADR-0004 Decision 1 for the
  `Instance` family, deletes the ADR-0005 P1 scaffold,
  `docs/adrs/0006-instance-replacement-view.md:374-381`).
  ADR-0004's Status is still "Proposed"; ADR-0006's Status is still
  "Draft. Measured; the maintainer accepts or rejects" (`:3`).
- **F close-out** (`docs/plans/2026-09-11-f-program-close-out.md`): F0-F3
  landed opt-in, F4 retired unclaimed (#1573). The reopening bar (>=10%
  relative total work share, one-family replacement view, all parity
  green, `:75-84`) was then **run and failed** (`:27-60`). Two loose ends
  remain open: ADR-0006's Draft status and the prototype's fate
  (`:48-50`), and `testtypeview.py` is referenced by no CI job
  (`:51-55`; re-verified today: `rg -i typeview .github/` matches
  nothing).
- **remaining-migration-plan.md**: Phase F closed (`:658-680`); Phase G4
  read-serving claimed per family then **parked by the #1624 correction**
  (`:835-867`); Phase H startability is blocked on the same storage-flip
  bar (`docs/plans/2026-09-13-phase-h-readiness-brief.md:246-254`).
- **#1624 correction** (`:835-867`, ledger entry
  `docs/plans/type-kernel-seam-ledger.md`, tail): the load-robust
  four-arm CPU gate on the cold self-check (production defaults,
  kernel-off, mirror-off, both-off; instructions retired, mean of three,
  spread <0.3%) measures the default-on native stack at **+201.7e9
  retired instructions (~+85% CPU)**: node mirror alone +84.3e9
  (+23% over Python), **type kernel alone +117.8e9 (+32%)** against the
  366.3e9 Python baseline. The kernel's own +32% is why upstream #1624
  is still OPEN.
- **G4 graduation criteria** (`docs/plans/2026-09-16-g4-graduation-criteria.md:63-81`):
  the #1836 rulings (read-serving suffices, per-family claims) are the
  shape any future Stage B claim would have to borrow: per family, with
  the write-path-still-Python binding condition stated.
- The fresh production survey (2026-09-19, `scripts/measure_native_share.py`
  + `misc/audit_wire_traffic.py`) establishes the per-call program complete:
  2,418,887 seam calls, 99.8% native decisions, 0.2% Python fallback, top
  deferral 0.09% of calls. Deferral reduction is exhausted as a lever,
  which is why the per-call seams cannot be the remaining rung.

The chain agrees on one conclusion across five documents: **the per-call
seam program is done, and every ownership rung that has been measured
(F1 capture, P2/P2b shadow, typeview replacement, G4 capture) measured
a loss or noise.** The only ownership rungs never charged at full cost
are the ones now parked, and they are parked because the charge (the
#1624 four-arm gate) is known: capture pays a crossing per tracked write.

## 2. What a Rust-owned Type proxy requires (against the real code)

ADR-0004 + ADR-0005's correction answer this completely; the requirements
that survived into the built code are:

- **Shadow, never replacement of the object**: live Python objects stay
  canonical; the store holds per-node handles plus data
  (`typeview.rs` entries keyed by `identity::handle_for_stable`, each
  entry pinning its object). `isinstance`, `__slots__`, plugin hooks and
  astmerge see live objects unchanged. ADR-0006 measured all four of
  ADR-0004's contract objections **satisfiable**
  (`docs/adrs/0006-instance-replacement-view.md:98-167`): plugins
  unaffected, astmerge unaffected, no cache participation, daemon handled
  by the standard `_clear_native_resolvers` reset (`mypy/build.py:2022-2028`).
- **Identity by key, not value**: `Instance.type` as a fullname,
  `TypeAliasType.alias` through the resolver map (ADR-0002 Decision 1).
- **Phase scoping**: no shadow is live during semanal/typeanal;
  the mutability boundary is the one `_type_wire_cache_enabled` already
  draws (`mypy/types.py:129-137`).
- **Coherence by epoch/touch + drop-on-write, never a write barrier**:
  ADR-0005's epoch counter (touch bumps it; stored entries miss), plus
  the `Instance.__setattr__` interceptor for the replacement arm
  (`mypy/typeview.py`).
- **Write-back via Python**: Rust decides, the shim mutates the live
  object, then drops the entry (ADR-0004 Decision 4).
- **Gate shape**: default-off, `None`-defers-to-Python, wired through the
  existing `Options`/module-global pattern (`mypy/options.py:412`,
  `native_type_kernel = True` default; per-seam `_set_native_*_active`
  wiring at `mypy/build.py:1102-1202`; per-module `_HAS_TYPE_KERNEL`
  import flags). The typeview prototype deliberately uses an env gate
  instead (no `Options` field, `mypy/build.py:1405-1416`), keeping it out
  of `OPTIONS_AFFECTING_CACHE` entirely.
- **Parity surface**: the gate-off/on differential must be byte-identical
  over `testcheck` + `testtypes` + the fine-grained family + the plugin
  corpus (`mypy_self_check.ini:22` loads `mypy.plugins.proper_plugin`)
  + a 0-error cold self-check.

None of these requirements is the blocker. The blocker is arithmetic.

## 3. The minimal first rung: it already exists, and it already ran

The commission asks for "one type variant (smallest viable: a leaf like
`NoneType`/`UninhabitedType`/plain `Instance`), behind a default-off
gate, with the Python fallback intact". That artifact is in the tree:

- Family: `Instance` (the largest-construction-volume family, the one
  ADR-0004's brief picked first,
  `docs/plans/2026-09-11-adr0004-proxy-brief.md:37-39`).
- Gate: `MYPY_TYPE_VIEW` env, 0/1/2, default off, no `Options` field
  (`mypy/typeview.py:110-111`, `mypy/build.py:1405-1416`).
- Fallback intact: the funnel probe returns `None` on any doubt and the
  pure-Python walk runs unchanged (`mypy/types.py:4769-4775`).
- Parity suite: 33 tests in `mypy/test/testtypeview.py` (gate-off/on
  funnel differential, byte parity for leaf/singleton/generic/two-arg/
  nested recursion), 13 Rust unit tests in `typeview.rs`.

Its go/no-go has already been run, twice, at increasing quality:

1. ADR-0006's counter leg (2026-09-15): baseline funnel 1.347s of
   parse+semanal+type_check 63.572s = **2.119% of total work**. The
   measured arm served 25.1% of funnel calls, removed 32.4% of walk
   encodes, cost **+1.67 GB RSS**, and the read-routing half was
   strictly negative (17,431,189 routed `args` reads, zero additional
   wire saving). Verdict: NO-GO by ~4.7x
   (`docs/adrs/0006-instance-replacement-view.md:179-265`).
2. The #1769 assessment (2026-09-16, upstream, closed): the same-day
   retirements cut the funnel's direct call counter 55%
   (2,839,692 -> 1,287,230 `serialize_calls`) while total work fell at
   most ~6.7%, so the ceiling is now **~0.9-1.1%** against the same 10%
   bar: gap ~9-11x (`docs/plans/2026-09-11-f-program-close-out.md:44-47`).

A leaf rung is dominated, not smaller-viable. ADR-0006 measured the
served instances as "the cheap ones": 18.0% of bytes for 25.1% of calls,
14.2 bytes per serve against 41.9 per walk write, and "leaf singletons
are two bytes" (`docs/adrs/0006-instance-replacement-view.md:228-237`).
A `NoneType`/`UninhabitedType` proxy addresses the two-byte tail of a
funnel that is itself ~1% of total work, while paying per-object
registration for every leaf constructed. There is no rung below
`Instance` that is not strictly worse on both numerator and denominator.

## 4. Go/no-go criterion, and the honest expected cost

Any Stage B storage rung must clear the bar the F close-out set
(`docs/plans/2026-09-11-f-program-close-out.md:75-84`):

- **Parity oracle (must be green regardless of perf)**: cold self-check
  `mypy_self_check.ini -n0 --no-incremental -p mypy -p mypyc` 0-error in
  both gate states; `testcheck`/`testtypes` gate-off/on differentials at
  exact baselines; the fine-grained family (fine-grained, cache, daemon,
  merge, diff); the plugin corpus. Byte-identity for served shapes is
  the typeview suite's existing differential.
- **Counter gate (settles the ceiling without any wall clock)**: PASS
  only if `serialize_view_bytes / (serialize_bytes + serialize_view_bytes)
  x funnel_share` can reach 10% on the head-stamped run
  (#1769's falsification plan, upstream). Expected today: ~0.9%, fail by
  ~10x, with `defers == 0` and ~+1.6 GB RSS.
- **Perf instrument (the load-robust A/B this repo adopted)**:
  `MYPY_NUM_WORKERS=0` cold self-check under `/usr/bin/env time -l`,
  three interleaved on/off pairs, instruction spread <0.3% per arm
  (the #1624 protocol, `docs/remaining-migration-plan.md:835-867`, LC_ALL=C).
  Wall clock only on a quiet host (bar load <5), via
  `scripts/measure_work_share.py`; the #1870 lane proved CPU/instruction
  pairs are the load-robust primary.

**Honest expected cost, from the parked-rung history:**

| rung | measured |
|---|---|
| F1 dual-write capture | +78.7s median, +85% (1.85x) |
| Proxy P2 lazy shadow | -0.5% wall, +5.6% CPU (a loss) |
| Proxy P2b thresholded | corpus A +8.97% wall; corpus B inside noise |
| typeview store+serve | +1.67 GB RSS, funnel share up (1.347s -> 1.852s) |
| typeview routed reads | strictly negative (17.43M pyO3 round-trips, zero saving) |
| node mirror capture (#1624) | +23% instructions over Python |
| type kernel itself (#1624) | +32% instructions over Python |

Every storage-side mechanism measured to date moved total work by at
most the funnel's ~1-2% ceiling, at a capture cost that exceeds it. The
commission's expected-cost answer is therefore not a range to beat but
a bound already in the ledger: **the entire addressable surface of any
proxy confined to the serialize funnel is <=1.1% of total work on the
current head, so the rung fails the 10% bar by construction.**

## 5. Blockers, risks, and what makes the rung fail

1. **The arithmetic blocker (decisive, documented three times).** A
   proxy serves seam calls by replacing per-call encode traffic. That
   traffic lives in the serialize funnel, measured at 2.119% of total
   work (ADR-0006) and ~0.9-1.1% after the 2026-09-16 retirements
   (#1769). No implementation quality changes the numerator's bound.
   ADR-0006 Consequence 4 retires the funnel as a hypothesis outright:
   "The remaining cost in `type_check_time` is elsewhere, and it is not
   addressable by type storage."
2. **Capture cannot be free.** Both shadow (epoch/touch on mutation
   sites) and replacement (`__setattr__` interception) arms pay per-object
   or per-write bookkeeping outside the funnel clock. Measured costs:
   +85% (F1), +5.6% CPU (P2), +1.67 GB RSS (typeview). The #1624
   correction generalized the finding: capture pays a crossing per
   tracked write, and that toll is what parks G4 read-serving.
3. **The read-serving half is measured strictly negative.** Routing
   Python attribute reads through a Rust store costs a PyO3 round-trip
   per access (17.43M reads, +5.8s) against a CPython slot read. This
   closes the "maybe reads are the win" branch the shadow arms left
   open.
4. **Program-external risk: the orphan prototype.** `typeview.rs` +
   `typeview.py` (1274 production lines) and `testtypeview.py` (33 tests)
   sit in the tree with no CI gate (`rg -i typeview .github/` matches
   nothing), an unresolved ADR status ("Draft. Measured; the maintainer
   accepts or rejects"), and a predecessor scaffold that was already
   deleted once for being an orphan ("wire it or delete it, do not leave
   a third orphan", `docs/adrs/0006-instance-replacement-view.md:376-381`).
   Rotting parity-unobserved native code is a correctness risk to the
   repo even with the gate off, because the funnel probe and the build.py
   wiring still execute on every production run (`mypy/types.py:4769`).
5. **If someone builds a leaf rung anyway**, the failure mode is
   silent: counters stay green, parity stays green, and the instruction
   A/B shows a small loss inside noise. That is exactly how P2b's
   corpus-B leg read before its ceiling was derived. The counter gate
   (item 4's ceiling test) is what converts that into a fast NO-GO
   without a corpus run.

## 6. Recommended disposition (the one actionable slice)

Do not build a new Stage B rung. The remaining actionable work in this
lane is the disposition the record already demands, and it is
falsifiable and cheap:

1. **Head-stamp the counters (one corpus run, no new code).** Rebuild
   the kernel into a scratch dir, then one cold self-check with
   `MYPY_SERIALIZE_STATS=1 MYPY_SERIALIZE_CLOCK=1 MYPY_TYPE_VIEW=1`,
   reading `typeview_encodes`, `typeview_defers`, `serialize_view*`,
   `serialize_calls`, `serialize_writes`, max RSS and the phase rows.
   Decision rule per #1769: PASS only if the served-bytes share times the
   funnel share can reach 10%. Expected: ~0.9%, NO-GO by ~10x, with the
   current head's figure recorded instead of the 09-15/09-16 artifacts
   (the newest artifact predates the last five retirements).
2. **Settle ADR-0006: Reject.** Change Status from "Draft" to
   "Rejected (measured NO-GO)" with the head-stamped figure. Per its
   Consequence 2, the prototype's fate is then deletion, not a successor:
   remove `crates/type_kernel/src/typeview.rs`, `mypy/typeview.py`,
   `mypy/test/testtypeview.py`, the funnel probe (`mypy/types.py:4766-4775`),
   and the build.py wiring (`:1405-1416`, `:2022-2028`, `:2336-2342`).
   The measurement instruments that survive it (`MYPY_SERIALIZE_STATS`,
   `MYPY_SERIALIZE_CLOCK`, `misc/audit_wire_traffic.py`) stay.
3. **Record the closure** in `docs/remaining-migration-plan.md` (Phase F
   text) and the claim ladder, so the next lane does not re-derive the
   NO-GO (this is #1769's follow-up 1, now done for the record but not
   for the prototype's status; do not repeat the drift).

## 7. What would reopen the line

ADR-0006's two-half table (`:321-330`) leaves exactly one unmeasured
shape: a mechanism that removes Python work **outside** the serialize
funnel, with no per-write capture. The reopening bar is therefore:

> A named mechanism, its addressable surface measured above 10% of
> total work on the cold self-check before any code is written, and its
> cost model free of per-object registration. The counter gate runs
> first; a design whose addressable surface cannot reach the bar by
> arithmetic is declined without a build.

Absent such a mechanism, E1 Stage B stays closed on ADR-0006's
measurement, and the migration's remaining honest line is #1624's own
+32% (making the type kernel cheaper than Python), not more storage.

## Sources verified against the tree on 2026-09-19

- `mypy/types.py:129-141` (wire cache flag), `:4763-4832` (funnel,
  view probe, clocked wrapper), `:5532`, `:5576` (ErasedType opt-in)
- `mypy/typeview.py` (gate arms `:22-24`, `:110-111`)
- `mypy/build.py:1102-1202` (native gate wiring), `:1405-1416`,
  `:2022-2028`, `:2336-2342` (typeview wiring/reset/stats)
- `mypy/options.py:412-421` (`native_type_kernel = True`,
  `native_type_mirror = False`, `native_type_instance_write = False`)
- `crates/type_kernel/src/typeview.rs` (871 lines),
  `crates/type_kernel/src/wire.rs:103` (ERASED_TYPE tag 122)
- `.github/`: no reference to typeview (re-verified)
- Upstream: #1671 CLOSED (experiment), #1769 CLOSED (assessment),
  #1624 OPEN (kernel +32%), #1836 CLOSED (G4 rulings)
