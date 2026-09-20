# Arithmetic operators slice: implementation brief (#150)

Preregistered spec for the next skeleton slice, fixed before any code. The
result matrix below was measured from the tree's own mypy
(`2.2.0+dev.19cfa68cc`, scratch extensions on `PYTHONPATH`) on 2026-09-20;
every supported arm's `.expected` is regenerated from that same mypy by
`misc/skeleton_codegen.py`, so the matrix is the contract, not a summary.

## Baseline

The skeleton rejects `int + int` today: `i + j` exits 2 with `binary
operations on these types are outside the supported subset`. `BinOpKind`
carries only `Add`, `Mult` and `Mod` (`crates/mypy-rs-skel/src/subset.rs:59`),
and the typing table (`src/check.rs:1968-2004`) covers `str + str`,
`float {+,*,%}` and `int % int` only. Any realistic module uses arithmetic.

## Result matrix (measured, `reveal_type`)

| expression | mypy result |
| --- | --- |
| `int + int`, `int - int`, `int * int`, `int // int`, `int % int` | `int` |
| `int / int` | `float` |
| `int + float`, `float - int`, `float * float`, `float / float`, `float // float`, `float % float` | `float` |
| `str + str` | `str` |
| `str * int`, `int * str` | `str` |
| `str % int` | `str` (no error) |
| `bool + bool` | `int` |
| `bool op int` | `int` |
| `-int`, `+int` | `int` |
| `-float` | `float` |
| `int < int`, `int <= float` | `bool` |
| `int ** int`, `float ** float` | `Any` |

## Error matrix (measured)

| expression | mypy diagnostic |
| --- | --- |
| `str - str` | `Unsupported left operand type for - ("str")  [operator]` |
| `str + int` | `Unsupported operand types for + ("str" and "int")  [operator]` |
| `int + str` | `Unsupported operand types for + ("int" and "str")  [operator]` |
| `float + str` | `Unsupported operand types for + ("float" and "str")  [operator]` |
| `str * str` | `Unsupported operand types for * ("str" and "str")  [operator]` |
| `int // str` | `Unsupported operand types for // ("int" and "str")  [operator]` |
| `-str` | `Unsupported operand type for unary - ("str")  [operator]` |

The message shape is decidable from the left operand alone: if it has no
dunder for the operator, mypy names only the left (`left operand type`);
otherwise it names both (`operand types`). Unary always names one.

## Scope decisions

1. **Supported operators:** `+`, `-`, `*`, `/`, `//`, `%` on `int`, `float`
   and `str` per the matrix, unary `-` and `+`, and augmented assignment for
   the same operators.
2. **`**` is excluded and gets an explicit unsupported arm.** Its mypy result
   is `Any`, and the skeleton has no `Any`-compatibility semantics, so
   accepting it would risk accepting programs mypy rejects (the #93 silent
   class). Revisit when `Any` lands.
3. **`bool` counts as `int`** in the operand table, with `int` results, as
   measured (`b + b` -> `int`). No new TypeInfo is needed: `builtins.bool`
   already sits in the closure.
4. **Operand errors are rendered byte-identically, not hard-rejected.** They
   are ordinary type errors inside the slice, and the three-way partition
   requires supported constructs to agree exactly. The current exit-2
   `"binary operations on these types are outside the supported subset"`
   path narrows to the genuinely out-of-subset cases (a non-instance
   operand, an unknown member on a corpora-owned record).
5. **Augmented assignment desugars to the binop plus the existing assignment
   check.** Verified: `i += f` on `i: int` yields mypy's
   `Incompatible types in assignment (expression has type "float", variable
   has type "int")  [assignment]`, and `s *= s` yields the `*` operator error.

## Deliverables

- `BinOpKind` gains `Sub`, `Div`, `FloorDiv`; `UnaryOpKind` gains `USub`,
  `UAdd`; `AugAssign` lowers to binop + assignment.
- The check table implements the result matrix and the three error message
  shapes exactly.
- Manifest arms, each with `.expected` from `misc/skeleton_codegen.py`:
  - supported: one arm per operator family across the matrix rows,
    `expressions.operator_unsupported_operands` and
    `expressions.operator_unsupported_left` for the error rows,
    `expressions.augmented_assignment`;
  - unsupported: `expressions.pow` (decision 2).
- Tests asserting both directions for each arm, plus the existing suites.

## Gates before merge

- `cargo test -q -p mypy-rs-skel --features skel`, `cargo fmt -- --check`,
  `cargo clippy -q -p mypy-rs-skel --all-targets --features skel -- -D warnings`.
- `misc/skeleton_upstream_gate.py`: `0 semantic-diff, 0 unclassified`.
- `misc/skeleton_codegen.py --check` clean.
- `misc/skeleton_duplication_probe.py` re-run and its verdict quoted in the PR
  (the primitives this slice reads are already duplicated; the probe is the
  record of whether the slice adds a new duplicated fact).
