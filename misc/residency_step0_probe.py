#!/usr/bin/env python3
"""#71 Lane 1 Step 0: probe run driver for the residency battery.

Runs the single-process cold self-check with the extended identity probe
(MYPY_SUBTYPE_IDENTITY_PROBE) and the in-kernel WirePhaseCounters on, so
one run yields the whole Step-0 counter set: pair- and operand-level
repeat populations, the family's serialize hit/miss split, the coded-call
count, and the family-attributed decode deltas.

The report is printed on every exit path (completed, failed, interrupted);
guards decide whether it may be read as evidence, never whether printed,
mirroring misc/wire_churn_phase_profile.py. Evidence is refused when the
run failed, the probe never engaged, the operand-class anomaly counter is
nonzero (a class the F3 predicate cannot explain), an id cap overflowed
(distinctness would silently undercount), the decode windows do not cover
exactly the coded calls (a skipped window would read as zero), the wire
phase counters are off (family decode deltas would read zero), or the
operand classification sum does not reconcile with op_appearances.

Usage: MYPY_SUBTYPE_IDENTITY_PROBE=1 .venv/bin/python misc/residency_step0_probe.py
Prereq: PYTHONPATH with the type_kernel and ast_serialize scratch dirs
(AGENTS.md, native build order).
Corpus class: /private/tmp/mypy-rs-sem.sh run 2 <this script>.
"""

from __future__ import annotations

import os
import sys

DEFAULT_ARGS = [
    "--config-file",
    "mypy_self_check.ini",
    "-n0",
    "--no-incremental",
    "--dump-build-stats",
    "-p",
    "mypy",
    "-p",
    "mypyc",
]

# WirePhaseCounters layout, shared with misc/wire_churn_phase_profile.py.
WIRE_COUNTER_NAMES = (
    "mode",
    "decode_calls",
    "decode_bytes",
    "decode_nodes",
    "encode_calls",
    "encode_bytes",
    "encode_nodes",
    "clone_nodes",
    "hash_bytes",
    "hash_ops",
)


def report(probe: dict[str, int], run_status: str, counters: tuple[int, ...]) -> None:
    print(f"[step0] run status: {run_status}", file=sys.stderr)
    for key in sorted(probe):
        print(f"[step0] {key:32s} {probe[key]}", file=sys.stderr)
    distinct = probe["op_first_seen"] + probe["op_recycled"]
    print(f"[step0] {'distinct_operand_objects':32s} {distinct}", file=sys.stderr)
    for name, value in zip(WIRE_COUNTER_NAMES, counters):
        print(f"[step0] {'wire_' + name:32s} {value}", file=sys.stderr)


def evidence_refusals(
    probe: dict[str, int], run_status: str, counters: tuple[int, ...]
) -> list[str]:
    wire = dict(zip(WIRE_COUNTER_NAMES, counters))
    reasons = []
    if run_status.startswith("FAILED"):
        reasons.append(f"run status {run_status!r} is not a completed check")
    if probe["entries"] == 0:
        reasons.append(
            "hollow probe: zero native-path entries (MYPY_SUBTYPE_IDENTITY_PROBE unset?)"
        )
    if probe["op_appearances"] != 2 * probe["entries"]:
        reasons.append(
            f"operand accounting: op_appearances {probe['op_appearances']} != "
            f"2 x entries {probe['entries']}"
        )
    classified = (
        probe["op_first_seen"]
        + probe["op_repeat"]
        + probe["op_recycled"]
        + probe["op_overflow"]
        + probe["op_unclassified"]
    )
    if classified != probe["op_appearances"]:
        reasons.append(
            f"operand classification sum {classified} != op_appearances "
            f"{probe['op_appearances']}"
        )
    if probe["op_class_anomaly"]:
        reasons.append(f"{probe['op_class_anomaly']} operand appearances no F3 class explains")
    if probe["op_overflow"] or probe["overflow"]:
        reasons.append("id cap overflowed: distinctness counters undercount")
    windows = probe["fam_windows"] + probe["fam_window_skipped"]
    if windows != probe["coded_calls"]:
        reasons.append(f"decode windows {windows} != coded calls {probe['coded_calls']}")
    if probe["fam_window_inconsistent"]:
        reasons.append(f"{probe['fam_window_inconsistent']} decode windows saw a reset")
    if wire["mode"] != 1:
        reasons.append(f"wire phase mode {wire['mode']} != 1: fam_decode_* deltas would read zero")
    return reasons


def main() -> int:
    # Worktree prelude (same as misc/wire_churn_phase_profile.py): the
    # shared .venv's editable finder maps `mypy` elsewhere (#1789), and
    # sys.path[0] is misc/. Pin this tree.
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if root not in sys.path:
        sys.path.insert(0, root)
    for finder in list(sys.meta_path):
        if "editable" in (type(finder).__module__ or "") or "editable" in repr(finder):
            sys.meta_path.remove(finder)
    os.environ["MYPY_NUM_WORKERS"] = "0"
    import mypy

    if not os.path.abspath(mypy.__file__).startswith(root + os.sep):
        print(f"[step0] mypy resolves outside this tree: {mypy.__file__}", file=sys.stderr)
        return 1
    import mypy.subtypes

    if not mypy.subtypes._identity_probe_on:
        print(
            "[step0] MYPY_SUBTYPE_IDENTITY_PROBE is off; this driver measures nothing",
            file=sys.stderr,
        )
        return 1
    import type_kernel

    if not all(
        hasattr(type_kernel, name)
        for name in (
            "rust_wire_phase_set_mode",
            "rust_wire_phase_reset",
            "rust_wire_phase_counters",
        )
    ):
        print("[step0] type_kernel lacks the wire phase seam: stale extension?", file=sys.stderr)
        return 1
    type_kernel.rust_wire_phase_set_mode(1)
    type_kernel.rust_wire_phase_reset()
    import mypy.main

    run_status: str | None = None
    try:
        mypy.main.main(args=list(DEFAULT_ARGS), clean_exit=True)
        run_status = "completed, mypy.main returned"
    except SystemExit as exc:
        if exc.code in (0, None):
            run_status = "completed, mypy exit 0"
        else:
            run_status = f"FAILED, mypy exit {exc.code!r}"
    except KeyboardInterrupt:
        report(
            mypy.subtypes._identity_probe,
            "INTERRUPTED, operator Ctrl-C",
            type_kernel.rust_wire_phase_counters(),
        )
        print("[step0] EVIDENCE REFUSED: run was interrupted", file=sys.stderr)
        raise
    except Exception as exc:
        run_status = f"FAILED, raised {type(exc).__name__}: {exc}"
        import traceback

        traceback.print_exc()
    probe = mypy.subtypes._identity_probe
    counters = type_kernel.rust_wire_phase_counters()
    report(probe, run_status, counters)
    if probe["fam_decode_calls"] != 2 * probe["coded_calls"]:
        print(
            f"[step0] note: fam_decode_calls {probe['fam_decode_calls']} != 2 x "
            f"coded_calls {probe['coded_calls']}: the windows caught in-crossing "
            f"decode traffic beyond the operand pair (consult re-entrancy)",
            file=sys.stderr,
        )
    refusals = evidence_refusals(probe, run_status, counters)
    for reason in refusals:
        print(f"[step0] EVIDENCE REFUSED: {reason}", file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main())
