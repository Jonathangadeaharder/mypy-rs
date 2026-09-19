#!/bin/bash
# #61 gap attribution: 3 interleaved paired rounds, kernel-on vs kernel-off,
# cold single-process self-check, instructions retired via /usr/bin/time -l.
# Sem pool corpus class: /private/tmp/mypy-rs-sem.sh run 2 misc/gap_attrib_rounds.sh
set -u
OUT=${GAP_ATTRIB_OUT:-/private/tmp/gap-attrib}
mkdir -p "$OUT"
PYK=PYTHONPATH-unset

run_arm() { # $1 round, $2 arm (on|off)
    local r=$1 arm=$2 flags=()
    if [ "$arm" = off ]; then flags+=(--no-native-type-kernel); fi
    local log="$OUT/round$r-$arm.log"
    {
        echo "=== round $r arm $arm start $(date '+%H:%M:%S')"
        uptime
    } > "$log"
    # bash 3.2 (macOS /bin/bash) errors on "${flags[@]}" under set -u when the
    # array is empty (the kernel-on arm), so guard the expansion.
    MYPY_NUM_WORKERS=0 \
    PYTHONPATH=/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver:/private/tmp/mypy-rs-local-typekernel \
    /usr/bin/time -l .venv/bin/python -m mypy --config-file mypy_self_check.ini \
        -n0 --no-incremental ${flags[@]+"${flags[@]}"} -p mypy -p mypyc \
        > "$OUT/round$r-$arm.out" 2> "$OUT/round$r-$arm.time" &
    local tpid=$!
    if [ "${GAP_ATTRIB_SAMPLE:-0}" = 1 ]; then
        # Attach the macOS sampler to the python process (time(1) execs
        # python, so the python pid is time's direct child). -mayDie makes
        # sample exit when the process dies.
        local pypid="" i=0
        while kill -0 "$tpid" 2>/dev/null; do
            pypid=$(pgrep -P "$tpid" | head -1)
            [ -n "$pypid" ] && break
            i=$((i+1)); [ "$i" -gt 300 ] && break
            sleep 0.1
        done
        if [ -n "$pypid" ]; then
            sample "$pypid" 3600 -mayDie -file "$OUT/round$r-$arm.sample.txt" >/dev/null 2>&1 &
            local sampid=$!
            wait "$tpid"
            local rc=$?
            # sample -mayDie exits on its own once python dies, but only
            # writes the output file on a natural exit: give it a moment
            # before killing, or the sample file is lost.
            local i=0
            while kill -0 "$sampid" 2>/dev/null; do
                i=$((i+1)); [ "$i" -gt 50 ] && break
                sleep 0.1
            done
            kill "$sampid" 2>/dev/null
        else
            echo "sample attach FAILED" >> "$log"
            wait "$tpid"
            local rc=$?
        fi
    else
        wait "$tpid"
        local rc=$?
    fi
    {
        echo "exit=$rc"
        uptime
        grep -E "instructions retired|cycles elapsed|real|maximum resident" "$OUT/round$r-$arm.time" || true
        tail -1 "$OUT/round$r-$arm.out"
    } >> "$log"
    echo "round $r arm $arm exit=$rc" >&2
}

for r in 1 2 3; do
    run_arm "$r" on
    run_arm "$r" off
done
echo "ALL ROUNDS DONE" >&2
