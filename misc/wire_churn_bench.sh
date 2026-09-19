#!/usr/bin/env bash
# Per-op instruction benches for the wire-churn falsifier (#63): one
# op-fixture per bench_wire_phase_op process under /usr/bin/time -l.
set -euo pipefail

BIN="${1:-}"
ITERS="${2:-1000000}"
REPS="${3:-3}"
if ! [[ "$ITERS" =~ ^[1-9][0-9]*$ && "$REPS" =~ ^[1-9][0-9]*$ ]]; then
    echo "iters and reps must be positive integers (got iters=$ITERS reps=$REPS)" >&2
    exit 1
fi
cd "$(git rev-parse --show-toplevel)"

if [[ -z "$BIN" ]]; then
    BIN=$(ls -t target/release/deps/type_kernel-* 2>/dev/null \
        | while read -r f; do [[ -x "$f" && "$f" != *.d ]] && echo "$f" && break; done)
fi
if [[ -z "$BIN" || ! -x "$BIN" ]]; then
    echo "no executable type_kernel release test binary under target/release/deps" >&2
    echo "run: cargo test --release -p mypy-type-kernel --no-run" >&2
    echo "usage: $0 [release-test-binary] [iters] [reps]" >&2
    exit 1
fi
echo "binary: $BIN  iters: $ITERS  reps: $REPS"

OUT=/private/tmp/wire-churn/bench_matrix.txt
mkdir -p "$(dirname "$OUT")"
: > "$OUT"

# Process-startup noise is ~+/-20M instructions, so every op runs REPS
# times and all derived costs use the per-op median.
OPS="control decode_drop decode_forget clone_forget clone_drop"
FIXTURES="any instance callable"
for rep in $(seq 1 "$REPS"); do
    for fixture in $FIXTURES; do
        for op in $OPS; do
            instr=$(WIRE_BENCH="$op-$fixture" WIRE_BENCH_ITERS="$ITERS" \
                /usr/bin/time -l "$BIN" --ignored --exact wire::tests::bench_wire_phase_op 2>&1 \
                | awk '/instructions retired/ {print $1}')
            if [[ -z "$instr" ]]; then
                echo "no instruction count for $op-$fixture (rep $rep)" >&2
                exit 1
            fi
            printf '%s %s %s %s\n' "$op" "$fixture" "$instr" "$rep" | tee -a "$OUT"
        done
    done
done

echo "--- per-node costs from per-op medians (instructions) ---"
awk -v iters="$ITERS" -v reps="$REPS" '
    { v[$1 "-" $2, $4] = $3 }
    function median(key,    i, j, t, m, a) {
        for (i = 1; i <= reps; i++) a[i] = v[key, i]
        for (i = 2; i <= reps; i++) {
            t = a[i]
            for (j = i - 1; j >= 1 && a[j] > t; j--) a[j + 1] = a[j]
            a[j + 1] = t
        }
        m = int((reps + 1) / 2)
        return (reps % 2) ? a[m] : (a[m] + a[m + 1]) / 2
    }
    END {
        split("any instance callable", fx, " ")
        nodes["any"] = 1; nodes["instance"] = 3; nodes["callable"] = 5
        for (i = 1; i <= 3; i++) {
            f = fx[i]
            n = nodes[f]
            dec = (median("decode_forget-" f) - median("control-" f)) / (iters * n)
            drp = (median("decode_drop-" f) - median("decode_forget-" f)) / (iters * n)
            clo = (median("clone_forget-" f) - median("control-" f)) / (iters * n)
            xdrop = (median("clone_drop-" f) - median("clone_forget-" f)) / (iters * n)
            printf "%-9s decode=%8.1f drop=%8.1f clone=%8.1f (clone_drop drop=%8.1f)\n", \
                f, dec, drp, clo, xdrop
        }
    }' "$OUT"
echo "raw matrix: $OUT"
