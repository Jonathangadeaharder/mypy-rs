#!/usr/bin/env python3
"""#81 Step 0: type-arena amortization probe (constructions vs appearances).

Runs the single-process cold self-check with runtime-only probes over the
immutable-leaf families (AnyType, UninhabitedType, NoneType, ErasedType,
DeletedType) and reports, per family: constructions, first and total
appearances at the serialize funnel `_serialize_type_for_visitor`
(mypy/types.py:4746; top-level calls plus nested writes inside a
funnel-initiated walk), and the refusal classes (the serve split that
prices the walk arm, plus accounting anomalies).

Everything is patched at runtime from this driver: no mypy/ file changes,
nothing engages unless this script is run. The appearance window opens
only inside a funnel call, so cache I/O (`mypy/cache_data.py`) and the
copy seam (`mypy/copytype.py`), which reach `_write_type_cached` or
`t.write` outside the funnel, are excluded by construction.

The report is printed on every exit path; guards decide whether it may be
read as evidence, never whether printed. Evidence is refused when the run
failed, the probe never engaged (zero funnel calls: every call site is
gated on the kernel seam), no family object was constructed or appeared,
an id was recycled among appearing objects, the funnel re-entered
mid-walk, or a family cache entry carried a tvar fingerprint (impossible
for a leaf: that would mean a misattributed appearance).

Usage: .venv/bin/python misc/arena_amortization_step0_probe.py
Pass extra mypy args (smoke runs only) as positional arguments.
Prereq: PYTHONPATH with the type_kernel, ast_serialize and module_resolver
scratch dirs (AGENTS.md, native build order); a probe run is never used as
an instruction baseline (the #71 Step-0 probe-pin discipline).
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

FAMILY_NAMES = ("AnyType", "UninhabitedType", "NoneType", "ErasedType", "DeletedType")
SERVE_CLASSES = ("hit", "miss", "cache_off", "tvar_reject")


def make_probe() -> dict:
    return {
        "funnel_calls": 0,
        "funnel_reentrant": 0,
        "id_recycled": 0,
        "family_fp_nonnone": 0,
        "constructions": {n: 0 for n in FAMILY_NAMES},
        "constructed_ids": set(),
        "appear_total": {n: 0 for n in FAMILY_NAMES},
        "appear_top": {n: 0 for n in FAMILY_NAMES},
        "appear_nested": {n: 0 for n in FAMILY_NAMES},
        "appear_first": {n: 0 for n in FAMILY_NAMES},
        "unconstructed_events": {n: 0 for n in FAMILY_NAMES},
        "unconstructed_ids": {n: set() for n in FAMILY_NAMES},
        "serve": {n: {c: 0 for c in SERVE_CLASSES} for n in FAMILY_NAMES},
        # id -> object, strong ref: pins appearing family objects so a
        # recycled id can never alias a live one (same discipline as the
        # F3 wire cache itself, mypy/types.py:133-136).
        "seen": {n: {} for n in FAMILY_NAMES},
    }


def install(probe: dict, types_mod, families: dict) -> None:
    window = [0]

    def record(name: str, t, nested: bool) -> None:
        key = id(t)
        seen = probe["seen"][name]
        prev = seen.get(key)
        if prev is not t:
            if prev is not None:
                probe["id_recycled"] += 1
            seen[key] = t
            probe["appear_first"][name] += 1
            if key not in probe["constructed_ids"]:
                probe["unconstructed_events"][name] += 1
                probe["unconstructed_ids"][name].add(key)
        probe["appear_total"][name] += 1
        probe["appear_nested" if nested else "appear_top"][name] += 1
        serve = probe["serve"][name]
        # Serve classes replicate the wrapped functions' decision order
        # exactly: phase gate first, then (nested only) the librt splice
        # availability, then the cache entry identity check.
        if not types_mod._type_wire_cache_enabled or (
            nested and types_mod.write_raw_bytes is None
        ):
            serve["cache_off"] += 1
            return
        entry = types_mod._type_wire_cache.get(key)
        if entry is not None and entry[0] is t:
            fp = entry[2]
            if fp is None:
                serve["hit"] += 1
            elif types_mod._tvar_fingerprint_valid(fp):
                serve["hit"] += 1
                probe["family_fp_nonnone"] += 1
            else:
                serve["tvar_reject"] += 1
        else:
            serve["miss"] += 1

    orig_funnel = types_mod._serialize_type_for_visitor

    def probed_funnel(t):
        probe["funnel_calls"] += 1
        if window[0]:
            probe["funnel_reentrant"] += 1
        name = families.get(type(t))
        if name is not None:
            record(name, t, nested=False)
        window[0] += 1
        try:
            return orig_funnel(t)
        finally:
            window[0] -= 1

    types_mod._serialize_type_for_visitor = probed_funnel

    orig_write = types_mod._write_type_cached

    def probed_write(t, data):
        if window[0]:
            name = families.get(type(t))
            if name is not None:
                record(name, t, nested=True)
        return orig_write(t, data)

    types_mod._write_type_cached = probed_write

    for cls, name in families.items():
        orig_init = cls.__init__

        def probed_init(self, *args, _orig=orig_init, _name=name, **kwargs):
            _orig(self, *args, **kwargs)
            probe["constructed_ids"].add(id(self))
            probe["constructions"][_name] += 1

        cls.__init__ = probed_init


def report(probe: dict, run_status: str, kernel_active: bool) -> None:
    err = sys.stderr
    print(f"[arena-step0] run status: {run_status}", file=err)
    print(f"[arena-step0] funnel calls: {probe['funnel_calls']}", file=err)
    print(f"[arena-step0] kernel visitor active at end: {kernel_active}", file=err)
    print(
        f"[arena-step0] anomalies: id_recycled {probe['id_recycled']}, "
        f"funnel_reentrant {probe['funnel_reentrant']}, "
        f"family_fp_nonnone {probe['family_fp_nonnone']}",
        file=err,
    )
    tot_c = tot_first = tot_app = 0
    for name in FAMILY_NAMES:
        c = probe["constructions"][name]
        first = probe["appear_first"][name]
        total = probe["appear_total"][name]
        top = probe["appear_top"][name]
        nested = probe["appear_nested"][name]
        unc = probe["unconstructed_events"][name]
        unc_ids = len(probe["unconstructed_ids"][name])
        serve = probe["serve"][name]
        ratio = f"{c / total:.3f}" if total else "inf (no appearances)"
        tot_c += c
        tot_first += first
        tot_app += total
        print(f"[arena-step0] {name:16s} constructions {c}", file=err)
        print(
            f"[arena-step0] {name:16s} appearances total {total} "
            f"(top {top} + nested {nested}), first {first}, "
            f"ratio constructions/total {ratio}",
            file=err,
        )
        print(
            f"[arena-step0] {name:16s} serve hit/miss/cache_off/tvar_reject "
            f"{serve['hit']}/{serve['miss']}/{serve['cache_off']}/"
            f"{serve['tvar_reject']}",
            file=err,
        )
        print(
            f"[arena-step0] {name:16s} refusal unconstructed appearances "
            f"{unc} events / {unc_ids} distinct (pre-arming objects)",
            file=err,
        )
    set_ratio = f"{tot_c / tot_app:.3f}" if tot_app else "inf (no appearances)"
    print(
        f"[arena-step0] family set: constructions {tot_c}, first {tot_first}, "
        f"total appearances {tot_app}, ratio {set_ratio}",
        file=err,
    )


def evidence_refusals(probe: dict, run_status: str) -> list[str]:
    reasons = []
    if not run_status.startswith("completed"):
        reasons.append(f"run status {run_status!r} is not a completed check")
    if probe["funnel_calls"] == 0:
        reasons.append("hollow probe: zero funnel calls (kernel seam off or patches bypassed)")
    if sum(probe["constructions"].values()) == 0:
        reasons.append("no family object constructed: nothing was measured")
    if sum(probe["appear_total"].values()) == 0:
        reasons.append("no family appearance at the funnel: nothing was measured")
    if probe["id_recycled"]:
        reasons.append(f"{probe['id_recycled']} id recyclings: distinctness undercounts")
    if probe["funnel_reentrant"]:
        reasons.append(f"{probe['funnel_reentrant']} re-entrant funnel calls mid-walk")
    if probe["family_fp_nonnone"]:
        reasons.append(
            f"{probe['family_fp_nonnone']} family entries with a tvar fingerprint: "
            "a leaf cannot carry one, appearance misattributed"
        )
    return reasons


def main() -> int:
    # Worktree prelude (same as misc/residency_step0_probe.py): the shared
    # .venv's editable finder maps `mypy` elsewhere (#1789), and sys.path[0]
    # is misc/. Pin this tree.
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if root not in sys.path:
        sys.path.insert(0, root)
    for finder in list(sys.meta_path):
        if "editable" in (type(finder).__module__ or "") or "editable" in repr(finder):
            sys.meta_path.remove(finder)
    os.environ["MYPY_NUM_WORKERS"] = "0"
    import mypy

    if not os.path.abspath(mypy.__file__).startswith(root + os.sep):
        print(f"[arena-step0] mypy resolves outside this tree: {mypy.__file__}", file=sys.stderr)
        return 1
    import mypy.types

    if not mypy.types._VISITOR_HAS_TYPE_KERNEL:
        print(
            "[arena-step0] type_kernel is not importable: the serialize funnel "
            "never fires without the kernel seam (scratch dirs on PYTHONPATH, "
            "AGENTS.md native build order)",
            file=sys.stderr,
        )
        return 1
    families = {}
    for name in FAMILY_NAMES:
        families[getattr(mypy.types, name)] = name
    probe = make_probe()
    install(probe, mypy.types, families)
    args = sys.argv[1:] or DEFAULT_ARGS
    import mypy.main

    run_status: str | None = None
    try:
        mypy.main.main(args=list(args), clean_exit=True)
        run_status = "completed, mypy.main returned"
    except SystemExit as exc:
        if exc.code in (0, None):
            run_status = "completed, mypy exit 0"
        else:
            run_status = f"FAILED, mypy exit {exc.code!r}"
    except KeyboardInterrupt:
        report(probe, "INTERRUPTED, operator Ctrl-C", mypy.types._native_visitor_active)
        print("[arena-step0] EVIDENCE REFUSED: run was interrupted", file=sys.stderr)
        raise
    except Exception as exc:
        run_status = f"FAILED, raised {type(exc).__name__}: {exc}"
        import traceback

        traceback.print_exc()
    report(probe, run_status, mypy.types._native_visitor_active)
    refusals = evidence_refusals(probe, run_status)
    for reason in refusals:
        print(f"[arena-step0] EVIDENCE REFUSED: {reason}", file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main())
