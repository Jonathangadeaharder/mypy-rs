#!/usr/bin/env python3
"""#63 Phase 1: kernel wire-churn phase profile on the cold self-check.

Enables the in-kernel counters (`type_kernel.rust_wire_phase_set_mode`),
runs the same single-process cold self-check corpus the #61 battery used
(`mypy_self_check.ini -n0 --no-incremental -p mypy -p mypyc`), and reports
the counters on stderr on every exit path. The counters are the falsifier's
deliverable: a probe that can exit silently is worse than no probe.

The report refuses to be read as evidence when the run failed or when the
counters are hollow (all zero = the seam never engaged), mirroring the
audit's evidence-refusal discipline (misc/audit_wire_traffic.py).

Usage: .venv/bin/python misc/wire_churn_phase_profile.py [-- extra mypy args]
Corpus class: /private/tmp/mypy-rs-sem.sh run 2 <this script>.
"""

from __future__ import annotations

import os
import sys

# (mode, decode_calls, decode_bytes, decode_nodes, encode_calls,
#  encode_bytes, encode_nodes, clone_nodes, hash_bytes, hash_ops)
# Order mirrors WirePhaseCounters10 in crates/type_kernel/src/wire.rs.
COUNTER_NAMES = (
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

DEFAULT_ARGS = [
    "--config-file",
    "mypy_self_check.ini",
    "-n0",
    "--no-incremental",
    "-p",
    "mypy",
    "-p",
    "mypyc",
]


def report(counters: tuple[int, ...], run_status: str) -> None:
    """Print the profile on every path that reaches here; guards decide
    whether it may be read as evidence, never whether it is printed."""
    print(f"[phase] run status: {run_status}", file=sys.stderr)
    for name, value in zip(COUNTER_NAMES, counters):
        print(f"[phase] {name:14s} {value}", file=sys.stderr)
    drops = counters[3] + counters[7]
    print(f"[phase] {'drop_nodes':14s} {drops} (decode+clone creation identity)", file=sys.stderr)


def evidence_refusals(counters: tuple[int, ...], run_status: str) -> list[str]:
    reasons = []
    if counters[0] != 1:
        reasons.append("mode is not 1: the profile was disabled mid-run")
    if run_status.startswith(("FAILED", "INTERRUPTED")):
        reasons.append(f"run status {run_status!r} is not a completed check")
    if counters[3] == 0:
        reasons.append("hollow profile: decode_nodes is 0 (seam never engaged)")
    return reasons


def main() -> int:
    # Worktree prelude (same as misc/gap_attrib_microbench.py): the shared
    # .venv is an editable install whose finder maps `mypy` to a possibly
    # deleted worktree (#1789), and sys.path[0] is misc/. Pin this tree.
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if root not in sys.path:
        sys.path.insert(0, root)
    for finder in list(sys.meta_path):
        if "editable" in (type(finder).__module__ or "") or "editable" in repr(finder):
            sys.meta_path.remove(finder)
    argv = list(sys.argv[1:]) or list(DEFAULT_ARGS)
    os.environ["MYPY_NUM_WORKERS"] = "0"
    import mypy

    if not os.path.abspath(mypy.__file__).startswith(root + os.sep):
        print(f"[phase] mypy resolves outside this tree: {mypy.__file__}", file=sys.stderr)
        return 1
    import type_kernel

    if not hasattr(type_kernel, "rust_wire_phase_set_mode"):
        print(
            "[phase] type_kernel lacks the phase profile seam: stale extension?", file=sys.stderr
        )
        return 1
    type_kernel.rust_wire_phase_set_mode(1)
    type_kernel.rust_wire_phase_reset()
    import mypy.main

    run_status: str | None = None
    try:
        mypy.main.main(args=argv, clean_exit=True)
        run_status = "completed, mypy.main returned"
    except SystemExit as exc:
        if exc.code in (0, None):
            run_status = "completed, mypy exit 0"
        else:
            run_status = f"FAILED, mypy exit {exc.code!r}"
    except KeyboardInterrupt:
        # A Ctrl-C is an operator action, not the run's own failure (#1846):
        # report the partial counters, never classify the abort as mypy's,
        # then re-raise.
        report(type_kernel.rust_wire_phase_counters(), "INTERRUPTED, operator Ctrl-C")
        raise
    except BaseException as exc:
        run_status = f"FAILED, raised {type(exc).__name__}: {exc}"
        import traceback

        traceback.print_exc()
    counters = type_kernel.rust_wire_phase_counters()
    report(counters, run_status)
    refusals = evidence_refusals(counters, run_status)
    for reason in refusals:
        print(f"[phase] EVIDENCE REFUSED: {reason}", file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main())
