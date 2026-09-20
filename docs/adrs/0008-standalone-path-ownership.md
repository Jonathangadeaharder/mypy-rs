# ADR-0008: The next ownership flip lives on the standalone path (skeleton first)

- Status: Proposed (design record for issue #83, epic #72; no production
  change in this lane). Enables the skeleton flip lane and its pre-registered
  Step-0 falsifier, both specified in the companion brief
  `docs/plans/2026-09-19-standalone-skeleton-brief.md`.
- Date: 2026-09-19
- Issue: #83 (chooses one of its three candidates and justifies the choice);
  epic #72 (roadmap steps 2-5; the skeleton is the first vertical cut through
  all four at trivial corpus depth).
- Supersedes: the *sequencing* premise of ADR-0007, not its architecture:
  ADR-0007 ordered ownership flips family by family inside the running
  Python hybrid; the measured evidence now forbids that sequencing. The
  arena/facade design itself is retained, repositioned as the eventual
  plugin-compatibility bridge for the standalone path (epic step 7).
- Does NOT supersede: ADR-0001, ADR-0003 (the wire codec remains the lingua
  franca of the hybrid and the cache), ADR-0006's measured rejection.

## Context

Three ownership experiments are falsified, each pre-registered and killed
before or without production code:

- **#54 / ADR-0006** (Instance replacement view): Python stays canonical,
  Rust holds derived field storage. 0.2-1.1% addressable ceiling, +501 MB
  RSS, capture and routed-read halves both measured negative.
- **#71** (subtype residency, stable TypeIds): Python stays canonical, Rust
  keeps a resident decoded copy. Repeat population too small to carry the
  gates (61.1% wire-deletion ceiling vs 95%; conversion ratio ceiling 0.43
  vs 0.5); 38.9% of operand appearances are first-sight objects whose first
  walk cannot be deleted while Python is canonical.
- **#81** (leaf-arena amortization, ADR-0007's own first slice): mint at
  construction adds a crossing on a hot path (95.78% of constructions never
  reach the serialize funnel; set ratio 19.61 vs the 1.0 gate).

The common mechanism, recorded on #72: every tested form keeps Python
canonical and gives Rust a second copy, so removed boundary work does not
convert to end-to-end savings, and the bookkeeping that replaces it
(capture, lookup, storage, per-construction mint) is paid on hot paths.
Issue #83's deciding constraint follows: the next slice must delete a
representation (Rust canonical, no copy), and any proposal that adds a
per-object crossing on a hot path is disqualified.

## Decision

**The next representation Rust owns is owned on a path where Python does
not run.** The next slice is the standalone-binary skeleton: a `mypy-rs`
executable that type-checks one fixed, trivial corpus end to end (parse,
bounded semantic analysis, kernel checks, diagnostics) with no `mypy` Python
import at check time. For that scope the ownership question is settled by
construction, not by amortization: the Python AST node graph, the Python
symbol tables and the PyO3 boundary never exist, so there is no second
copy to keep coherent and no per-object crossing to price. This is issue
#83's candidate 2, and it carries candidates 1 and 3 inside it (see
Decision 2).

### Decision 1: no hybrid-resident representation flip is the next slice

Evaluated against the deciding constraint, each hybrid-resident candidate
reproduces the falsified mechanism:

- **Rust-canonical AST in the hybrid (candidate 1).** Today the native
  parse builds a Rust tree inside `parse`
  (`crates/ast_serialize/src/lib.rs:535`, ruff-based), serializes it
  (`serialize_suite`, `crates/ast_serialize/src/lib.rs:778`), and drops
  the tree; Python retains the wire bytes (`FileRawData`) and fully
  materializes `mypy.nodes` objects for every checked module
  (`mypy/build.py:4613`; `imports_only` is true only in the parallel
  parent, and the worker materializes fully). Semantic analysis consumes
  the entire tree, so the Python node graph is not deletable while Python
  drives semanal. Retaining the Rust tree alongside it adds a second
  resident copy that deletes nothing (the #71 shape). Replacing byte
  decode with cheaper node construction makes the copy cheaper, which the
  deciding constraint excludes. And the AST wire bytes are load-bearing
  beyond node construction: they are the fine-grained cache artifact
  (`mypy/build.py:6755`, `raw_data.write(buf)`) and the parallel-parent
  handoff, so the byte path cannot be deleted in the hybrid either.
- **Rust semanal for a bounded subset in the hybrid (candidate 3).** Its
  products (symbol tables, scopes, TypeInfo facts) are consumed by the
  Python checker on every read, so either each read crosses (the #81
  disqualification) or the products materialize as Python objects and the
  Python copy stands (the #54/#71 disqualification). The kernel's
  TypeInfo snapshot store is itself fed from Python
  (`crates/type_kernel/src/typeinfo.rs:155`, built per pass by reading the
  live Python TypeInfo graph), so a Rust-side producer inside the hybrid
  would shadow a Python-owned graph it cannot replace.
- **What remains.** The only bounded scope in which a representation is
  actually deleted is a scope from which the Python host is absent. That
  is the skeleton.

### Decision 2: the skeleton is the bounded home of candidates 1 and 3

- Candidate 1's substance (Rust-canonical AST with Rust-driven traversal)
  is what the skeleton is: the parser's Rust tree drives the check for its
  corpus, and Python reads nothing because Python is not there. The G4
  failure was mirroring a Python-owned AST; the skeleton owns the only AST
  that exists on its path.
- Candidate 3's substance (Rust semanal for a bounded subset: module symbol
  tables and scopes) is built inside the skeleton, where its products have
  exactly one consumer, the Rust driver. In the hybrid the same work would
  be a second copy; in the skeleton it is the only copy. The bounded-subset
  claim is itself the pre-registered Step-0 question (brief section 5):
  the skeleton is GO only if the typeinfo read-closure its trivial corpus
  actually consults is a small, nameable fraction of what a real check
  consults. If it is not, no bounded skeleton exists and the honest next
  lane is a Rust typeinfo-from-stubs producer, specified then with its own
  falsifier.

### Decision 3: what the skeleton reuses, and what it does not touch

- Reused as shipped: the ruff-based parse machinery, the native resolver,
  and the kernel's Rust internals. The kernel crate is already dual
  `["cdylib", "rlib"]` (`crates/type_kernel/Cargo.toml`), so the skeleton
  consumes it as a library and calls Rust functions directly. It adds no
  PyO3 seam and retires none; the 961 registered seams
  (`docs/plans/type-kernel-seam-ledger.md`, #1677 planner count; the epic
  text's "830" is an earlier rounded figure) stay constant in the
  production check, and the skeleton path executes zero of them.
- Untouched: all of `mypy/`, all `Options`, `OPTIONS_AFFECTING_CACHE`,
  `CACHE_VERSION`, the daemon, the plugin contract. The skeleton is a new
  feature-gated crate and binary, never in a wheel, never in the check
  path. Production mypy does not change by one byte.
- Bootstrap honesty: the skeleton's TypeInfo facts come from generated
  fixtures committed as data. The generator may run Python mypy as a
  codegen-time tool (the same status typeshed stubs have); the check-time
  claim "no Python interpreter" is the milestone, and retiring the
  codegen-time dependency is later semanal work, stated as such in the
  brief, not hidden.

### Decision 4: what this means for ADR-0007

ADR-0007's architecture (arena, `TypeId`, facades, write-through) is not
rejected; its sequencing is. Hybrid-resident flips are closed by
measurement in their cheap forms (#81 for eager mint; #71 for residency;
ADR-0006 for storage moves). The facade design is retained as the bridge a
future standalone path will need for Python-written plugins (epic step 7):
at that point Python is the interop layer, not the owner, which is the one
arrangement ADR-0007's Decision 2 contract always assumed.

## Consequences

- The flip lane (brief section 9) builds the skeleton; the Step-0 lane
  (#85, self-assigned) measures the bootstrap read-closure first and can
  kill the shape before any skeleton code, mirroring how #81 killed the
  leaf flip and #71 killed residency.
- Performance is judged by milestone semantics per #72: the skeleton is
  not required to be net-positive against anything; its milestone value is
  the first check path in the project with a single representation, and
  the first place `I(Rust standalone)` versus `I(upstream mypy)` is
  measurable at all (trivial scope now, the endgame question the epic
  says is the only meaningful one later).
- The hybrid stays the shippable artifact and the correctness oracle: the
  skeleton's differential gate is byte-identical diagnostics against
  Python mypy on the same fixed corpus (brief section 7).
- Roadmap steps 2-4 (Node ownership, traversal ownership, semanal) now
  grow inside the standalone path, slice by slice, rather than inside the
  hybrid. The strangler-fig rule survives with a corrected grafting
  point: new ownership grafts onto the Python-free path; the hybrid only
  receives deletions of things the standalone path has absorbed.

## Status note (2026-09-20)

Traversal ownership within this ADR's meaning is already satisfied at module
granularity: the `mypy-rs` executable schedules every checking pass over a
source module in Rust (`Driver::check_main`,
`crates/mypy-rs-skel/src/main.rs`), with semantic facts still arriving from
fixtures, which the milestone allows. The reader verdicts of 2026-09-20
(recorded in `docs/remaining-migration-plan.md`) therefore treat the hybrid
as frozen reference implementation and differential oracle only, and order
the next work as: language slices, then the stdlib-stubs producer on the
first duplicated stdlib fact (#151), then the first standalone performance
gate at the first no-handwritten-record module (#152), with full semantic
analysis deferred until a producer demonstrably needs transformations.
