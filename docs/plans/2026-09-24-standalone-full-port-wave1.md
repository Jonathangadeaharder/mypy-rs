# Standalone full port, wave 1: lift the ported kernel, grow the driver

Date: 2026-09-24. Status: active plan for the wave-1 lanes. Epic #72 (full-port
definition of done), ADR-0008 (the next representation Rust owns is owned on a
path where Python does not run).

## Why this wave exists

The census of `crates/type_kernel/src` (126 modules, 171,719 lines) says the
migration's hard part is already written: 992 `#[pyfunction]` seam wrappers sit
on top of 219 `*_inner` functions and ~2,546 private helpers, and only 32 of
those inner functions mention a pyo3 type in their signature. The shape is
uniform:

```rust
#[pyfunction]
pub(crate) fn rust_X(bytes: &[u8]) -> PyResult<Option<Vec<u8>>> {
    let t = decode_type(bytes)?;      // wire decode: hybrid-only cost
    X_inner(&t, ...).map(encode_type) // wire encode: hybrid-only cost
}

fn X_inner(t: &Type, ...) -> Option<T> { /* the ported logic, pure Rust */ }
```

`X_inner` operates on `wire::Type`, `typeinfo::TypeResolver`,
`TypeInfoSnapshot` — pure Rust values with no Python behind them. So the
standalone path does not need mypy re-ported; it needs the existing Rust logic
**reached without the seam wrapper**. That is what `standalone/` is.

Re-porting the same logic inside `crates/mypy-rs-skel` would duplicate 171k
lines and fork the semantics from the oracle. It is forbidden by this plan.

## Architecture

```
ruff parse -> port::lower  (complete AST lowering)
           -> port::stubs  (typeshed producer) + port::semanal (binding, scopes,
                            symbol tables, type-expression analysis)
           -> records      (TypeInfoSnapshot / resolver, Python-free)
           -> port::{expr,call,member,stmt,narrow,pattern,infer}
                            over type_kernel::standalone::*
           -> port::diag   (byte-identical mypy messages)
           -> port::deps   (dependency + incremental state)
```

Two mirrored module trees, both scaffolded empty in this commit:

- `crates/type_kernel/src/standalone/<area>.rs` — the public Rust API of one
  area. Lifts the existing inner logic; **no pyo3 type in any public
  signature**, no seam registration, no change to any hybrid code path.
- `crates/mypy-rs-skel/src/port/<area>.rs` — the skeleton-side driver glue for
  the same area. Adapts skeleton records to the kernel API and the kernel's
  answers back to diagnostics. Owns no checking logic of its own.

Areas: `member`, `call`, `infer`, `types`, `narrow`, `stmt`, `semanal`, `diag`,
`deps`, `pattern`, `expr`, `records` (kernel) and the same plus `lower`,
`stubs` (skeleton). Each area file is owned by exactly one lane for the whole
wave; the `mod.rs` files and the two crate roots are owned by this scaffolding
commit and are append-only afterwards.

## Wave-1 lane set (8 lanes)

| lane | kernel area | skeleton area | first deliverable |
|---|---|---|---|
| semanal | `standalone/semanal.rs` | `port/semanal.rs` | name binding, scopes, symbol tables, type-expression analysis reachable without Python |
| records+stubs | `standalone/records.rs` | `port/stubs.rs` | the typeshed producer (#151): parse `builtins.pyi`/`typing.pyi`, retain reachable records, delete the hand-built fixture path for the covered facts |
| expr | `standalone/expr.rs` | `port/expr.rs` | operators, comprehensions, f-strings, literal/container expressions |
| call | `standalone/call.rs` | `port/call.rs` | actual-to-formal mapping, arity, overloads, callable compatibility |
| member | `standalone/member.rs` | `port/member.rs` | attribute access, properties, descriptors, class vs instance |
| infer | `standalone/infer.rs` | `port/infer.rs` | constraints, solving, substitution, expansion, freshening |
| types | `standalone/types.rs` | `port/types.rs` | union/join/meet, type ops, alias expansion, subtype exposure |
| stmt | `standalone/stmt.rs` | `port/stmt.rs` | assignment, control flow, defs, return, with, try |

Wave 2 (already scoped, not dispatched): `narrow`, `diag`, `deps`, `pattern`,
`lower`, the `check.rs` decomposition into pass modules, the driver integration
that wires `port::*` into `Driver::check_main`, and the corpus/manifest growth
that measures the result (#167, #168).

## Rules for every lane

1. **Read before writing.** Every lifted function must be read in its existing
   module first; copy the real signature and the real types. Inventing an API
   is the one failure mode this plan cannot survive, because nothing is
   compiled locally.
2. **No local builds, tests or scripts.** CI (`pr-gate`, `merge-battery`) is
   the compiler. Write small, obviously-correct increments, push, and fix from
   the CI log. Never weaken an invariant to go green.
3. **Lift, do not duplicate.** If the logic exists in the kernel, expose it;
   if it exists only in Python (`mypy/*.py`), port it into the area module and
   name the Python source function in the doc comment.
4. **Python-free public surface.** No `pyo3` type in a public signature, no
   `PyResult`, no wire encode/decode in the standalone call path. Where the
   hybrid passes a Python callback, take a Rust trait object or an explicit
   record instead, and document the substitution.
5. **The hybrid is frozen.** No seam registration changes, no `Options` field,
   no `CACHE_VERSION`, no daemon/plugin change, no edit to `mypy/`. Production
   mypy does not change by one byte.
6. **semantic-diff = 0.** Supported behaviour matches mypy byte for byte;
   unsupported behaviour rejects loudly naming the construct. A wrong answer is
   never allowlisted.
7. **Unit tests live in your own area file** (`#[cfg(test)] mod tests`). They
   are the only thing exercising the lifted API before integration, and they
   keep `dead_code`/clippy clean in CI. A test must fail when the behaviour it
   pins is removed; say in the PR body which line that is.
8. **Stay in your files.** Another lane owns everything else. If your change
   needs a file outside your scope, report the need instead of editing it.

## Integration contract (wave 2 reads this)

Each area module must end the wave exposing, at minimum:

- the operations the skeleton's `Driver` needs for its pass, named after the
  mypy function they come from (`mypy/checkmember.py::analyze_member_access`
  and friends), so the differential against the oracle stays readable;
- a `Records`-style input type or a reference to `typeinfo::TypeResolver`, so
  no area invents its own semantic-fact store;
- `#[cfg(test)]` coverage of at least the happy path and one rejection path.

The wave-2 integration lane wires `port::*` into `Driver::check_main`, flips
manifest capabilities from `unsupported` to `supported` one arm at a time, and
holds the corpus partition gate at SEMANTIC_DIFF = 0 / UNCLASSIFIED = 0.

## What "done" means for the whole migration

Epic #72, verbatim: the main executable is Rust; parsing, semantic analysis,
type representation, checking, dependency management, diagnostics and
incremental state are Rust-owned; ordinary checking requires no Python
interpreter; the upstream mypy test corpus has essentially complete behavioral
parity; diagnostics are identical or the differences are documented; Python
plugins run through an optional compatibility bridge; no Python fallback
participates in a normal check.

Progress against it is measured by the manifest census
(`crates/mypy-rs-skel/testdata/manifest.json`, 47 capabilities: 24 supported /
23 unsupported at `afc2b8232`) and the upstream corpus partition
(`testdata/upstream_seed.json`, 13 cases at `afc2b8232`), never by lines of
Rust written.
