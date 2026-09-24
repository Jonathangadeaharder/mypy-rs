# Handoff: strangler-fig Rust migration loop

## RESUME POINT — 2026-09-24, evening (standalone full-port wave 1 closed: 11 PRs merged, #194 in flight)

Objective: the full Rust port, per `docs/plans/2026-09-24-standalone-full-port-wave1.md`
and `docs/adrs/0008-standalone-path-ownership.md`. Wave 1 lifted kernel areas
behind Python-free `standalone::*` APIs plus `mypy-rs-skel` `port::*` adapters.
The hybrid stays frozen; semantic-diff = 0; nothing under `mypy/` moved.

`main` = `63c649b45` (after #183/#188/#189 landed; #194 expected to land on
top). The prior seam-deferral resume point (wave 12, 2026-09-18) is in git
history at the parent of `4f9b49cb8`.

### Merged this wave

| merge SHA | PR | area (issue) | what |
|---|---|---|---|
| `c817dae6e` | #170 | scaffold | port module tree scaffolded, wave-1 contract fixed (`b048a2fb0`, `a55c93c09`) |
| `4f9b49cb8` | #178 | expr (#172) | expr kernel behind a Python-free API |
| `baba6f13b` | #179 | semanal (#171) | semanal type-query and lookup API |
| `004851fd6` | #180 | infer (#175) | infer constraint and solve API |
| `2b2d769bb` | #182 | stmt (#177) | statement-checking kernel API |
| `c1dd95c0d` | #184 | types (#176) | type algebra without the pyo3 seam |
| `38d99f245` | #185 | call (#173) | call dispatch and argument-mapping API |
| `274843017` | #186 | expr (2nd) | format-string kernel exposed to the driver |
| `7a169c0bf` | #183 | member (#174) | member kernel behind a Python-free API |
| `21d070e97` | #188 | records (#151, 1/3) | standalone record layer for produced type facts |
| `63c649b45` | #189 | semanal (2nd) | base-class and metaclass resolution |

PR #194 (types increment 2: typeops family + accessors + `aliases.rs` lift)
was green except a `cargo fmt` failure; the one-line fix is pushed
(`05c8f7897 style(standalone): take rustfmt's breaks in the types module`) —
merge it once the four required checks are green.

### Harvest (dead lanes, this wave)

Five lanes died on model-access (403) errors before writing handoffs; two
left uncommitted increment-2 work that CI never saw. Everything is under
`/private/tmp/mypy-rs-wave1-harvest/`:

- `HANDOFF-standalone-types.md`, `HANDOFF-standalone-stmt.md`,
  `HANDOFF-standalone-records-stubs.md` — full lane handoffs (condensed below).
  Note: `docs/HANDOFF-*.md` matches `.git/info/exclude`, so these were never
  committable without `-f`; they live on disk only.
- `semanal-increment2-uncommitted.patch` — +565/−50 across
  `standalone/semanal.rs` (+452) and `typeanal_{callable,info,special}.rs`,
  from the `mypy-rs-parity-gaps` worktree (`feat/standalone-semanal`).
- `infer-increment2-uncommitted.patch` — +424/−7: a new
  `standalone/infer.rs` body (+417) and small touches in
  `applytype/erase_typevars/freshen/unify`, from `mypy-rs-perf-gate`
  (`feat/standalone-infer`).

Both patches are unreviewed, uncompilable-as-proven, and rebase onto main at
your own risk: diff them against the current `standalone/semanal.rs` /
`standalone/infer.rs` on `main` first — `#189` and `#180` may already
contain overlapping content. Treat them as candidate material, not truth.

### CI / merge protocol (unchanged)

- Four required checks: `lint-changed`, `pr-gate`, `codeql`, `merge-battery`.
  `parity*` runs on the PR but is advisory context for these pure-lift lanes.
- `ocr-review` queues forever (self-hosted runner offline, #181); ignore it.
- `mergeable_state: BEHIND` is constant (main moves fast); do NOT rebase a
  green PR solely to clear it — squash-merge with `--squash --admin` instead,
  or you burn a ~10 min `merge-battery` round.
- Push to the `github` remote, never `origin` (Forgejo mirror).
- `pr-gate` stops at the first failing step: a fmt failure means clippy,
  tests, the skel crate, and the corpus gate never ran (three lanes lost a
  round to exactly this).
- rustfmt: `max_width` 100, `fn_call_width` 60, `chain_width` 60,
  `struct_lit_width` 18, `reorder_imports`/`reorder_modules` on.
  Pre-commit hook: max 3 consecutive `//` lines, 88-char comments.
- Never hand-write an `.expected` file. Never edit `skeleton_api.rs` from an
  area lane; never edit `standalone/mod.rs` / `port/mod.rs` (scaffold already
  declares all modules; append-only).

---

## Condensed handoff: types (#176) — branch `feat/standalone-types`, PR #184 merged + #194 in flight

Landed (increment 1 merged; increment 2 = #194): 32+ public fns in
`crates/type_kernel/src/standalone/types.rs` (`make_union` :81 …
`tuple_length` :524, each lifting a named kernel core), the
`aliases.rs` visibility lift (`TypeAliasSnapshot`, `AliasTvar`,
`TypeAliasResolver` pub at `aliases.rs:28,55,83`, re-exported
`standalone/types.rs:67` — unblocks ten `checkexpr_functions.rs` entries),
and `crates/mypy-rs-skel/src/port/types.rs`: `Declined` :54,
`describe` :73, `Algebra<'a>` :144 (join/meet/union/narrow/overlaps/
subsumes/same/supertype_args/proper/simple_literal/tuple_fallback/
expand_sum_type, all fallible → `Result<_, Declined>`).

Remaining (ranked):
1. **Alias installation** — largest gap. `TypeResolver::install_aliases`
   (`typeinfo.rs:277`) and `aliases` (`typeinfo.rs:269`) stay `pub(crate)`;
   standalone callers can build a resolver but not attach it, so every
   alias operand declines in 13 ops. Unblock edit (records lane, #195):
   `pub fn install_alias_snapshots(&self, map: Arc<HashMap<String, TypeAliasSnapshot>>)`
   beside `typeinfo.rs:277`.
2. **Increment 3** — tuple/callable set ops + predicates, each a
   `fn` → `pub(crate) fn` widening in owned files: `join_tuples`
   (`setops.rs:5981`), `meet_tuples` (`:6240`), `safe_join` (`:1719`),
   `safe_meet` (`:1779`), `try_contracting_literals_in_union` (`:3828`),
   `is_type_var_like` (`meet.rs:29`), `is_literal_type_like`
   (`typeops.rs:683`), `contains_recursive_alias` (`setops.rs:4051`),
   `is_type_obj_callable` (`:2115`), `join_instances` (`:5669`, wrap with
   local `seen` via `pub(crate) SeenInstances` at `setops.rs:5619`).
   `safe_join` uses `setop_result_to_type`, which declines on `Encoded` —
   document or give it a `materialize_join`-based sibling.
3. **Truthiness family** — `typeops::true_or_false` (`typeops.rs:488`)
   returns `pub(crate) TruthinessResult`; no Rust materializer exists
   (`truthiness_to_py` needs an interpreter). `true_only`/`false_only`
   (`typeops.rs:397/:434`) read live `TypeInfo`; need snapshot-backed
   substitutes. New logic, own increment.
4. `NativeTypeResolver::from_resolver` (`typeinfo.rs:1503`) is
   `#[cfg(test)]`-only — not an escape hatch for anyone.

Negative controls, traps, and the full remaining list are in
`/private/tmp/mypy-rs-wave1-harvest/HANDOFF-standalone-types.md`.

Contract: infer calls `join_types`/`meet_types` (take `&SubtypeContext`,
materialized), `make_simplified_union`, `make_union`, `is_subtype`,
`is_same_type`, `is_equivalent`, `is_more_precise`,
`map_instance_to_supertype`, `erase_to_bound`, `get_type_vars`,
`tuple_fallback`, `bind_self`, `is_recursive_pair`. `None` always means
decline, never "no result". Integration lane (#190) wires
`Algebra::new(&self.ctx, &self.resolver)` into `Driver` via `check.rs:194`
(`SubtypeContext`) and `check.rs:171` (`TypeResolver`).

Noticed, not fixed (no issues filed): stale `RefCell` doc `aliases.rs:80-81`;
stale module doc `aliases.rs:1-22`; three `SetOpResult` converters with
different coverage (`setops.rs:1369`, `:909`, `:3783`); `Bottom` pinned
`ambiguous: true` in two converters vs `strict_optional`-branching
`meet.rs:1808`; `SubtypeContext::new`/`with_callable_flags` `pub(crate)`
though fields are pub (`subtypes.rs:329`, `:353`); `is_overlapping_types`
uses a depth cap (`meet.rs:26`, `MAX_DEPTH = 200`) where mypy uses
`seen_types`.

---

## Condensed handoff: stmt (#177) — PR #182 merged; issue stays open

Landed: 17 public ops in `crates/type_kernel/src/standalone/stmt.rs`
(:153-787, supporting enums/records :62-771) and 7 in
`crates/mypy-rs-skel/src/port/stmt.rs` (`StmtContext` :42, `MemberProbe`
:64, `return_stmt_facts` :162, `check_return` :181,
`check_annotated_assignment` :257, `name_lvalue_facts` :310,
`self_attr_lvalue_facts` :322, `op_has_inplace_method` :333,
`operator_assignment_method` :349 — the latter two ported from
`mypy/operators.py:37-51`). Visibility flips in `checker_stmts.rs` (7 items)
and `checker_functions.rs` (39 items: 29 decision-tag consts + 10 classify
fns), additive only. Issue #196 filed: `skip_definition` conjunction needs
one source of truth (`checker_functions.rs:2061-2099`).

Remaining (ranked):
1. **Increment 2: function-definition checking** (unblocked):
   `check_for_missing_annotations` (core `classify_missing_annotations`
   `checker_functions.rs:2605`), `check_unbound_return_typevar` (:6249),
   `check_final_without_value` (:3853), `check_compatibility_final_super`
   (:72), `check_compatibility_classvar_super` (:177),
   `check_compatibility_all_supers` (:237), `check_new_signature` (:411),
   `check_func_def_override` (:446), `check_explicit_override_decorator`
   (:2276), `is_writable_attribute` (:1342), `check_match_args` (:1597),
   `is_valid_defaultdict_partial_value_type` (:1646), `is_private`/
   `is_dunder`/`is_sunder` (:51/:56/:61). Notes:
   `classify_missing_annotations` takes `&TypeResolver` +
   `&TypeAliasResolver` and calls `generators.rs` fns (expr lane's module —
   coordinate, do not edit); the `#[allow(dead_code)]` on two `TYPE_TAG_*`
   const groups (`:2582-2586`) gets dropped here; do not lift the
   `#[cfg(test)]` `is_valid_inferred_type_inner` (`checker_stmts.rs:454`).
2. **Increment 3: class defs + truthiness**: `check_enum_new` (:517),
   `check_enum_bases` (:606), `check_enum` (:653), `check_getattr_method`
   (:768), `check_metaclass_compat` (:1174), truthiness needs a
   `MemberProbe`-shaped port (`instance_is_truthy` :3438,
   `dispatch_truthy_tag` :3450 liftable; `is_truthy_type` :3468 takes
   `&PyAny`), `classify_isinstance_head` (:5787, already `pub(crate)`,
   belongs to the narrow lane — coordinate).
3. **Blocked**: #196 edit (kernel parity suites needed); `AliasContext`
   can only be empty until the records lane ships a public alias record
   (`standalone/stmt.rs:69` gets the constructor then);
   `constant_fold.rs` not liftable (folds via CPython `operator`).
   Wave brief's #136/#137 conditional-control-flow claim is FALSE on this
   base: `subset.rs:198` `BodyStmt` has no `with`/`try`/`raise` arms and
   `check.rs::check_seq` handles only `If` — the with/try/raise/
   reachability kernel ops have no skeleton caller yet.

Contract for #190: entry points `check_return`,
`check_annotated_assignment`, `operator_assignment_method` (needs
`MemberProbe` over `ClassModel` or a `TypeResolver` snapshot), plus the fact
fns; both checking entry points return `Result<Option<Diagnostic>,
CheckError>`; driver builds one `StmtContext` per statement (borrows all
four `Driver` fields, no cloning); swap targets are `check.rs::check_seq`
arms `Return`/`LocalAnnAssign`/`AugAssign` (1535-1690) and the
missing-return block of `check.rs::check_body` (1499-1533). Deferral
contract: `None` = decline; the caller must produce a loud
`CheckError::Input` naming the construct, never a pass.

Full details: `/private/tmp/mypy-rs-wave1-harvest/HANDOFF-standalone-stmt.md`.

---

## Condensed handoff: records + stubs (#151, increment 1 of 3) — PR #188 merged

Landed: `crates/type_kernel/src/standalone/records.rs` (~1980 lines):
`ClassFacts` :252 (the input record), `ModuleFacts` :390,
`SymbolFact`/`SymbolScope`/`SymbolRef` :350/:429/:446, `RecordStore` :663
(`insert_class` :903, `resolver()` :877, queries :718-870), private C3/
metaclass/abstract/protocol/promotion walks (:1020-1405), 34 tests.
`misc/skeleton_codegen.py` now also emits
`crates/mypy-rs-skel/testdata/producer_golden.json` (records for the seven
producer-owned builtins), guarded by the existing `--check` pr-gate step.

Remaining (ranked):
1. **The producer** (`port/stubs.rs`, unblocked): parse embedded
   `builtins.pyi` via `include_str!` +
   `ruff_python_parser::parse_unchecked_source(src, PySourceType::Stub)`,
   fill `ClassFacts`/`ModuleFacts`, `RecordStore::insert_class`, prove
   equality against `producer_golden.json`. One edit in `fixtures.rs`
   (owned): factor `Fixtures::load`'s record loop into
   `pub fn decode_records(json: &str)`.
2. **`builtins.str`** — blocked on cross-module records, dependency order
   `builtins.type` → `abc.ABCMeta` → typing.Sequence/Reversible/
   Collection/Iterable/Container (`typing.pyi:544,555,667,673,678`) → str.
   `RecordStore` refuses today with `UnresolvedBase { class: "builtins.str",
   base: "typing.Sequence" }` — ready-made test.
3. **`port/lower.rs`** (file exists, empty): adapter `crate::subset` →
   `ClassFacts`/`ModuleFacts`. Generic base args (`class NamedBox(Sized[T])`,
   `testdata/class_slice.py:21`) need class-frame tvar copies from
   `check.rs` — take them as input or refuse loudly. Pin
   `RecordStore::insert_class(lower::class_facts(...))` ==
   `model::snapshot(...)` for the four corpus classes.
4. **Delete the fixture path** (wave 2, needs `check.rs`): replace
   `fixtures.resolver`/`fixtures.builtins_symbols` in `Driver` with
   `store.resolver()` + produced `ModuleFacts`, then drop `fixtures.rs`,
   `skeleton_fixtures.json`, and the `PRODUCER_OWNED` split in
   `misc/skeleton_codegen.py:83`.
5. **Three visibility edits outside scope** (→ #195): `mro.rs:99`
   `linearize` → `pub(crate)` (removes a throwaway-resolver clone in
   `records.rs:1057`); `semanal_metaclass.rs:210`
   `classify_recalculate_metaclass_inner` → `pub(crate)` (removes the
   restated rule `records.rs:1130`); `semanal_classprop.rs:14`
   `TYPE_PROMOTIONS` → `pub(crate)` (removes the restated table
   `records.rs:616`).
6. `skeleton_api.rs` re-exports wanted (may not self-edit):
   `pub use crate::standalone::records::{ClassFacts, ModuleFacts,
   RecordError, RecordStore, SymbolFact, SymbolKind};`.

Producer design facts (do not re-derive): class ranges in
`mypy/typeshed/stdlib/builtins.pyi` — object :101, type :182, int :253,
float :366, str :487, bool :930, function :1049; only
`sys.version_info <op> (major, minor)` and `sys.platform == "..."` are
decidable version gates (refuse `==`/`!=` against a 2-tuple — mypy compares
the full 5-tuple); member counts object 23 / type 25 / int 59 / bool 9 /
function 14; `@disjoint_base` has no snapshot field — carried in
`ClassFacts.disjoint_bases`, never faked. Escalations that fire: aliases
depending on analysis state (`_LiteralInteger`), cross-module MRO ordering
(topological sort until a cycle), class-level cycles need `PlaceholderNode`
+ re-analysis (the line where the extractor stops being an extractor).

Contract for #190: build `RecordStore`, `insert_module` each produced
module, `insert_class` base-first (bases must pre-exist; cycles/duplicates/
unlinearizable refused — caller owns order), hand `store.resolver()` to
`standalone::{expr,types,member,call,infer}` (`standalone/expr.rs:186`
takes `&TypeResolver`).

Full details: `/private/tmp/mypy-rs-wave1-harvest/HANDOFF-standalone-records-stubs.md`.

---

## Reconstructed entries — lanes that died without a handoff

These five lanes hit 403 model-access failures before the wind-down
directive could complete. No handoff files exist; entries below are from the
coordinator's git/CI evidence, not lane self-reporting.

### expr (#172) — PR #178 merged; lane closed cleanly before wind-down
`standalone/expr.rs` + `port/expr.rs` on main. Second increment (#186,
format-string kernel) also merged. `checkexpr_functions.rs` entries that
take `&TypeAliasResolver` are blocked on the types-lane aliases lift
(landed in #194). Lane reported nothing outstanding.

### semanal (#171) — PRs #179 + #189 merged; increment 2 uncommitted
Merged: type-query/lookup API (#179) and base-class + metaclass resolution
(#189). Increment 2 (died mid-flight, never CI'd): harvested patch —
+452 lines in `standalone/semanal.rs`, `typeanal_{callable,info,special}.rs`
touch-ups. Re-derive against current main before trusting it.

### call (#173) — PR #185 merged; worktree clean
Call dispatch + argument-mapping API on main (`standalone/call.rs` +1703,
`port/call.rs`). Worktree (`mypy-rs-infra`) left clean; no known remainder.

### infer (#175) — PR #180 merged; increment 2 uncommitted
Merged: constraint + solve API. Increment 2 (died mid-flight): harvested
patch — a new `standalone/infer.rs` body (+417) plus small
applytype/erase_typevars/freshen/unify touch-ups. Consumes the types-lane
API (join/meet/subtyping); `aliases.rs` lift (#194) unblocks part of it.

### member (#174) — PR #183 merged (by coordinator after lane died)
Lane ran 205 tool calls, opened #183, then died on 403. PR was green on
`2ba0ac25e` (all four checks), merged at `7a169c0bf`. Two known parity gaps
live in this area: #160 (hierarchy-cycle participant) and #162 (attribute
whose definer lands only in pass 3b) — both need the member API's facts,
which is exactly what #183 landed. No handoff was written; the PR body and
`standalone/member.rs` on main are the source of truth.

### integration (#190) — scaffold only; no lane PR
`port/mod.rs` declares all 13 port modules (some still `#[allow(dead_code)]`
shells: `lower.rs`, `stubs.rs`, `narrow.rs`, `pattern.rs`, `diag.rs`,
`deps.rs` have no wired content). The lane died before either deliverable
(check.rs decomposition, then wiring) landed. Worktree `mypy-rs-census`
left clean at main. The wiring job is: swap `Driver::check_seq` arms to
`port::stmt`/`port::types`/`port::member`/`port::call` per the contracts
above, and build the `RecordStore` pipeline. Nothing is wired yet.

---

## Wave 2, in order (open issues)

1. **#194** — merge types increment 2 once green (fmt fix pushed).
2. **#195** — the three unblock edits (`typeinfo.rs:277` alias install;
   `mro.rs:99`, `semanal_metaclass.rs:210`, `semanal_classprop.rs:14`) —
   small, unblocks types/records/infer remainder.
3. **#190** — wire the lifted port API into `Driver::check_main` /
   `check_seq` (the integration lane).
4. **#191 / #192 / #193** — narrow, diag, lower areas (new lanes).
5. **#196** — `skip_definition` single source of truth (kernel parity).
6. **#151** increments 2-3 — producer + `builtins.str` + delete fixture path.
7. **#177 / #176 / #171 / #175** — area issues stay open until their
   remaining increments land.

### Rules that bit this wave (carry forward)

- `pr-gate` stops at the first failing step — fmt failure silently skips
  clippy/tests/corpus; read the step name before diagnosing.
- `fn_call_width = 60` (not 100): bind operands to short locals before
  `assert_eq!`. `chain_width` only applies at two or more chain items.
- Remote is `github`, never `origin`. Squash-merge `--admin` is normal here
  (branch protection non-strict); a green PR stays mergeable while BEHIND.
- `standalone/mod.rs` / `port/mod.rs` are shared append-only rebase
  hotspots; area lanes never edit them.
- Heredoc PR bodies mangle backticks — use `--body-file`.
- Never print whole CI logs or full diffs; redirect to `/private/tmp` and
  read bounded slices.
- Scratch under `/private/tmp`, never in a worktree; `docs/HANDOFF-*.md`
  is in `.git/info/exclude`, so per-area handoffs on disk are invisible to
  git status — that is why they are summarized here instead.
