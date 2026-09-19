#!/bin/bash
# #61 gap attribution: cProfile arms (kernel-on/off), one run each, under
# /usr/bin/time -l (hook cost calibration). tottime is hook-inflated: used
# for per-function DELTA attribution and exact ncalls, never for totals.
set -u
OUT=${GAP_ATTRIB_OUT:-/private/tmp/gap-attrib}
mkdir -p "$OUT"

run_arm() { # $1 arm (on|off)
    local arm=$1 flags=()
    # bash 3.2 (macOS /bin/bash) errors on "${flags[@]}" under set -u when the
    # array is empty (the kernel-on arm), so guard the expansion.
    if [ "$arm" = off ]; then flags+=(--no-native-type-kernel); fi
    MYPY_NUM_WORKERS=0 \
    PYTHONPATH=/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver:/private/tmp/mypy-rs-local-typekernel \
    /usr/bin/time -l -o "$OUT/cprofile-$arm.time" \
    .venv/bin/python -m cProfile -o "$OUT/cprofile-$arm.prof" -m mypy \
        --config-file mypy_self_check.ini -n0 --no-incremental --no-fast-exit \
        ${flags[@]+"${flags[@]}"} \
        -p mypy -p mypyc > "$OUT/cprofile-$arm.out" 2> "$OUT/cprofile-$arm.err"
    echo "arm $arm exit=$? size=$(stat -f%z "$OUT/cprofile-$arm.prof")" >&2
}

run_arm on
run_arm off
echo "CPROFILE ARMS DONE" >&2
