# First standalone performance gate: preregistration (#152)

Preregistered before the gate can run. The milestone the gate measures
does not exist yet: the first source module checked end to end using a
real stdlib producer and no hand-written semantic records. Until the
producer (#151) and a producer-backed module exist, nothing here runs;
this record fixes the method and the numbers so the run, when it happens,
is a measurement and not a post-hoc story.

## Milestone trigger

The gate fires at the first module satisfying all three:

1. checked by the standalone `mypy-rs` executable with no `mypy` Python
   import and no libpython linkage (the ADR-0008 invariant, asserted by
   the existing gates test);
2. whose required TypeInfo facts come from the real producer, not from
   `crates/mypy-rs-skel/fixtures/skeleton_fixtures.json` (a module is
   producer-backed when the fixture carries no record for a fact it reads);
3. inside the supported subset (every construct it uses is a `supported`
   manifest arm).

The module is named when it is ready; this prereg does not fix which
module, only what "the first one" means.

## Arms

- **A. upstream Python mypy** on this tree, scratch native extensions on
  `PYTHONPATH`, `--no-incremental -n0`, `mypy_self_check`-equivalent flags,
  the module plus its dependencies.
- **B. standalone Rust, warm.** The `mypy-rs` executable on the module
  after the producer has built and cached its records for the module's
  dependency set.
- **C. standalone Rust, cold.** The same with the producer record cache
  emptied before the run, so the first-run producer cost is isolated
  (reader: producer cost may dominate the first run and amortize later).

## Measurement

- **Instructions and RSS:** `/usr/bin/time -l` on each arm, read
  `instructions retired` and `maximum resident set size` from the stderr
  report, median of 3 reps. Same technique as
  `misc/blob_marshal_bench.py`.
- **Wall-clock:** recorded for the record only, never a gate input
  (the shared machine load confounds it).
- **Semantic-record stats (arms B/C, from the producer):** records
  produced, records retained/reachable, module/type lookups.

## Preregistered gate (deliberately loose)

GO iff all three hold:

1. `I_B < 2 * I_A` (median retired instructions, arm B vs arm A);
2. `RSS_B < 2 * RSS_A` (peak resident set, arm B vs arm A);
3. arm B's diagnostics are byte-identical to arm A's.

Arm C is reported (never gated): its delta against B is the
amortization figure.

## On a NO-GO

Record the arm that failed, the break point, and the semantic-record
stats. If the failure is arm C's cold producer cost (i.e. B is fine, C is
not), that is expected and the gate still passes on B; note it. If B
fails clause 1 or 2, the standalone architecture has a real scaling
regression and this prereg's record becomes the input to whichever
optimization follows. Do not tighten the gate to make it pass; do not
relax it to make it pass.

## Relationship to the old hybrid gate

This is the first performance gate measured on the destination
architecture rather than the hybrid. The hybrid's parity question is
closed (the ownership experiments are falsified); this gate asks the
question the hybrid could never answer: how does `I(Rust standalone)`
scale against `I(upstream mypy)` once the Python representation is gone.
The numbers here and the `+82.4e9` hybrid gap are not comparable and
must never be placed in the same table.
