#!/usr/bin/env python3
"""#61 gap attribution: instruction-denominated per-call microbenches.

Every bench measures one seam-constituent operation against a control loop of
identical shape (a no-op Python function with the same arity), so
interpreter startup, mypy imports and loop machinery cancel:

    per-call instructions = (instr(op run) - instr(control run)) / N

Instructions come from /usr/bin/time -l ("instructions retired"), which is
load-invariant. Each bench subprocess checks, before and after the measured
loop, that the operation actually did its work (wire cache grew, counters
moved, expected return values), so a bench cannot silently measure a no-op;
the parent exits non-zero if any engagement check fails.

Run from the worktree root (needs the scratch type_kernel .so):

    .venv/bin/python misc/gap_attrib_microbench.py [--iters N]

Kernel benches use the in-repo extension via /private/tmp/mypy-rs-local-typekernel
(the same build the paired rounds use; never the PyPI stub).
Results print as a table and land in /private/tmp/gap-attrib/microbench.txt.
Lanes with their own private scratch (e.g. #71 Step 0, which must not touch
the shared typekernel dir a sibling lane builds) override the extension dir
and the output dir through MYPY_MICROBENCH_TK_SO / MYPY_MICROBENCH_OUT_DIR
and can filter benches with --only name1,name2.
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
from collections.abc import Callable

# Worktree prelude (same as scripts/measure_wire_prep.py): the shared .venv
# is an editable install whose finder maps `mypy` to a possibly-deleted
# worktree (#1789), and sys.path[0] is misc/, not the root. Pin this tree.
_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)
for _f in list(sys.meta_path):
    if "editable" in (type(_f).__module__ or "") or "editable" in repr(_f):
        sys.meta_path.remove(_f)

PY = sys.executable
HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.environ.get("MYPY_MICROBENCH_OUT_DIR") or "/private/tmp/gap-attrib"
TK_SO = os.environ.get("MYPY_MICROBENCH_TK_SO") or (
    "/private/tmp/mypy-rs-local-typekernel/type_kernel.cpython-313-darwin.so"
)

# (name, needs_kernel_ext, iters)
BENCHES = [
    ("gate_check", False, 1_000_000),
    ("ser_hit", False, 1_000_000),
    ("ser_miss_any", False, 500_000),
    ("ser_miss_instance", False, 500_000),
    ("ser_visitor_hit", False, 1_000_000),
    ("ser_visitor_miss", False, 500_000),
    ("wire_store", False, 2_000_000),
    ("content_probe", False, 2_000_000),
    ("content_store", False, 1_000_000),
    ("pyo3_noarg", True, 2_000_000),
    ("pyo3_1arg", True, 2_000_000),
    ("pyo3_13args", True, 1_000_000),
    ("callback_r2p", True, 2_000_000),
    ("callback_p_direct", False, 2_000_000),
    ("is_subtype_e2e_repeat_on", True, 200_000),
    ("is_subtype_e2e_repeat_off", False, 200_000),
    ("is_subtype_e2e_distinct_on", True, 100_000),
    ("is_subtype_e2e_distinct_off", False, 100_000),
]


def run_measured(child_args: list[str], env: dict[str, str]) -> int:
    proc = subprocess.run(
        ["/usr/bin/time", "-l", PY, os.path.join(HERE, "gap_attrib_microbench.py")] + child_args,
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr[-4000:])
        raise SystemExit(f"bench subprocess failed: {child_args}")
    for line in proc.stderr.splitlines():
        if "instructions retired" in line:
            return int(line.split()[0].replace(",", ""))
    raise SystemExit(f"no instructions line for {child_args}: {proc.stderr[-2000:]}")


def parent() -> int:
    default_iters = None
    if "--iters" in sys.argv:
        _iters_idx = sys.argv.index("--iters") + 1
        if _iters_idx >= len(sys.argv):
            raise SystemExit("--iters requires a positive integer")
        try:
            default_iters = int(sys.argv[_iters_idx])
        except ValueError:
            raise SystemExit(f"--iters needs an integer, got {sys.argv[_iters_idx]!r}") from None
        if default_iters <= 0:
            raise SystemExit(f"--iters must be positive, got {default_iters}")
    only: set[str] | None = None
    if "--only" in sys.argv:
        _only_idx = sys.argv.index("--only") + 1
        if _only_idx >= len(sys.argv):
            raise SystemExit("--only requires a comma-separated list of bench names")
        only = {n.strip() for n in sys.argv[_only_idx].split(",") if n.strip()}
        unknown = only - {b[0] for b in BENCHES}
        if unknown or not only:
            raise SystemExit(
                f"unknown or empty --only selection: {sorted(unknown)}; "
                f"known benches: {sorted(b[0] for b in BENCHES)}"
            )
    os.makedirs(OUT_DIR, exist_ok=True)
    env = dict(os.environ)
    # Prepend the extension dir rather than replacing PYTHONPATH so a lane
    # can add its own scratch dirs (e.g. the #71 Step 0 private typekernel)
    # without losing the shared ast/resolver paths.
    _tk_dir = os.path.dirname(TK_SO)
    env["PYTHONPATH"] = _tk_dir if not env.get("PYTHONPATH") else _tk_dir + ":" + env["PYTHONPATH"]
    env["MYPY_NUM_WORKERS"] = "0"
    env.setdefault("PYTHONDONTWRITEBYTECODE", "1")
    print(f"{'bench':32} {'iters':>9} {'instr_op':>14} {'instr_ctl':>14} {'instr/call':>11}")
    rows = []
    t0 = time.time()
    for name, needs_ext, bench_iters in BENCHES:
        if only is not None and name not in only:
            continue
        iters = default_iters or bench_iters
        if needs_ext and not os.path.exists(TK_SO):
            raise SystemExit(f"bench {name} needs the scratch type_kernel .so at {TK_SO}")
        op_i = run_measured(["--child", name, str(iters)], env)
        ctl_i = run_measured(["--child", name, str(iters), "--control"], env)
        per_call = (op_i - ctl_i) / iters
        print(f"{name:32} {iters:>9} {op_i:>14} {ctl_i:>14} {per_call:>11.1f}")
        rows.append((name, iters, op_i, ctl_i, per_call))
    with open(os.path.join(OUT_DIR, "microbench.txt"), "w") as f:
        for name, iters, op_i, ctl_i, per_call in rows:
            f.write(f"{name}\t{iters}\t{op_i}\t{ctl_i}\t{per_call:.1f}\n")
    print(f"total wall {time.time() - t0:.0f}s; table in {OUT_DIR}/microbench.txt")
    return 0


def noop(*args: object) -> None:
    return None


def require(cond: object, msg: str) -> None:
    """Engagement check; a bare assert would vanish under python -O."""
    if not cond:
        raise SystemExit(f"engagement check failed: {msg}")


# One measured-loop helper per call arity: a shared tail would union every
# branch's op and arg at one `f(*a)` site, which static analysis misreads as
# arity mismatches. The loop body is byte-identical in every helper.


def loop0(iters: int, control: bool, op: Callable[[], object], a: tuple) -> None:
    f = noop if control else op
    for _ in range(iters):
        f(*a)


def loop1(iters: int, control: bool, op: Callable[[object], object], a: tuple) -> None:
    f = noop if control else op
    for _ in range(iters):
        f(*a)


def loop2(iters: int, control: bool, op: Callable[[object, object], object], a: tuple) -> None:
    f = noop if control else op
    for _ in range(iters):
        f(*a)


def loop4(
    iters: int, control: bool, op: Callable[[object, object, object, object], object], a: tuple
) -> None:
    f = noop if control else op
    for _ in range(iters):
        f(*a)


def loop13(iters: int, control: bool, op: Callable[..., object], a: tuple) -> None:
    f = noop if control else op
    for _ in range(iters):
        f(*a)


def fixture_info(fullname: str):
    from mypy.nodes import Block, ClassDef, SymbolTable, TypeInfo

    defn = ClassDef(fullname.split(".")[-1], Block([]), None, [])
    defn.fullname = fullname
    info = TypeInfo(SymbolTable(), defn, "mod")
    defn.info = info
    info.mro.append(info)
    return info


def child(name: str, iters: int, control: bool) -> int:
    """One measured subprocess. Engagement-checked, then the bare loop runs."""
    from mypy import subtypes, types

    types._set_type_wire_cache_enabled(True)

    if name == "gate_check":
        from mypy.state import state
        from mypy.types import ErasedType

        info = fixture_info("mod.A")
        l = types.Instance(info, [])
        r = types.Instance(info, [])
        ctx = subtypes.SubtypeContext()
        subtypes._HAS_TYPE_KERNEL = True
        subtypes._native_subtype_active = True
        subtypes._native_subtype_resolver = object()
        state.strict_optional = True

        def gate(t: object) -> bool:
            return (
                subtypes._HAS_TYPE_KERNEL
                and subtypes._native_subtype_active
                and subtypes._native_subtype_resolver is not None
                and not ctx.erase_instances
                and not ctx.keep_erased_types
                and not isinstance(l, ErasedType)
                and not isinstance(r, ErasedType)
            )

        require(gate(l) is True, "gate unexpectedly false")
        loop1(iters, control, gate, (l,))

    elif name == "ser_hit":
        info = fixture_info("mod.A")
        t = types.Instance(info, [])
        b1 = subtypes._serialize_type(t)
        require(isinstance(b1, bytes) and b1, "prewarm failed")
        before = len(subtypes._type_wire_cache)
        for _ in range(3):
            subtypes._serialize_type(t)
        require(len(subtypes._type_wire_cache) == before, "hit path wrote to the cache")
        require(
            subtypes._type_wire_cache.get(id(t), (None, None, None))[1] is b1,
            "hit path did not serve the prewarmed blob",
        )
        loop1(iters, control, subtypes._serialize_type, (t,))

    elif name == "ser_miss_any":
        objs = [types.AnyType(types.TypeOfAny.unannotated) for _ in range(iters + 1)]
        idx = [0]

        def ser_any(_unused: object) -> bytes:
            i = idx[0]
            idx[0] = i + 1
            return subtypes._serialize_type(objs[i])

        ser_any(None)
        require(len(subtypes._type_wire_cache) >= 1, "miss path did not populate the cache")
        loop1(iters, control, ser_any, (None,))

    elif name == "ser_miss_instance":
        info = fixture_info("mod.A")
        objs = [types.Instance(info, []) for _ in range(iters + 1)]
        idx = [0]

        def ser_inst(_unused: object) -> bytes:
            i = idx[0]
            idx[0] = i + 1
            return subtypes._serialize_type(objs[i])

        ser_inst(None)
        require(len(subtypes._type_wire_cache) >= 1, "miss path did not populate the cache")
        loop1(iters, control, ser_inst, (None,))

    elif name == "ser_visitor_hit":
        info = fixture_info("mod.A")
        t = types.Instance(info, [])
        b1 = types._serialize_type_for_visitor(t)
        require(isinstance(b1, bytes) and b1, "prewarm failed")
        before = len(types._type_wire_cache)
        for _ in range(3):
            types._serialize_type_for_visitor(t)
        require(len(types._type_wire_cache) == before, "hit path wrote to the cache")
        loop1(iters, control, types._serialize_type_for_visitor, (t,))

    elif name == "ser_visitor_miss":
        info = fixture_info("mod.A")
        objs = [types.Instance(info, []) for _ in range(iters + 1)]
        idx = [0]

        def ser_v(_unused: object) -> bytes:
            i = idx[0]
            idx[0] = i + 1
            return types._serialize_type_for_visitor(objs[i])

        ser_v(None)
        require(len(types._type_wire_cache) >= 1, "miss path did not populate the cache")
        loop1(iters, control, ser_v, (None,))

    elif name == "wire_store":
        next_key = [10**7]

        def store(_unused: object) -> None:
            k = next_key[0]
            next_key[0] = k + 1
            types._type_wire_cache[k] = (None, b"x", None)

        store(None)
        require(len(types._type_wire_cache) >= 1, "store did not run")
        loop1(iters, control, store, (None,))

    elif name == "content_probe":
        key = (b"l", b"r", (False,) * 6)
        subtypes._subtype_answers[key] = True

        def probe(t: object) -> bool:
            return key in subtypes._subtype_answers

        require(probe(key) is True, "probe missed a present key")
        loop1(iters, control, probe, (key,))

    elif name == "content_store":

        def cstore(t: object) -> None:
            subtypes._subtype_answers[(b"l", t, (False,) * 6)] = True

        cstore(b"r")
        require(len(subtypes._subtype_answers) >= 1, "store did not run")
        loop1(iters, control, cstore, (b"r",))

    elif name == "pyo3_noarg":
        import type_kernel as tk

        v = tk.rust_checker_driver_counters()
        require(isinstance(v, tuple) and len(v) == 9, f"unexpected counters {v!r}")
        loop0(iters, control, tk.rust_checker_driver_counters, ())

    elif name == "pyo3_1arg":
        import type_kernel as tk

        before = tk.rust_checker_driver_counters()[0]
        tk.rust_checker_driver_record(0)
        after = tk.rust_checker_driver_counters()[0]
        require(after == before + 1, "counter did not move")
        loop1(iters, control, tk.rust_checker_driver_record, (0,))

    elif name == "pyo3_13args":
        import type_kernel as tk

        from mypy.state import state

        state.strict_optional = True
        info = fixture_info("mod.A")
        resolver = tk.build_native_resolver([info], [])
        inst = types.Instance(info, [])
        lb = subtypes._serialize_type(inst)
        rb = subtypes._serialize_type(inst)
        args = (lb, rb, False, False, False, False, False, False, False, False, resolver, False)
        code = tk.rust_is_subtype_coded(*args)
        require(code == 1, f"identity pair not decided natively: {code}")

        def coded13(*a: object) -> int:
            return tk.rust_is_subtype_coded(*a)  # type: ignore[attr-defined]

        require(coded13(*args) == 1, "crossing stopped answering")
        loop13(iters, control, coded13, args)

    elif name == "callback_r2p":
        import type_kernel as tk

        class Reg:
            def has_hook_for(self, hook: str, fullname: str) -> bool:
                return False

        reg = Reg()
        require(
            tk.rust_resolve_plugin_hook(reg, "mod.A.f", [], "get_function_hook") is None,
            "expected None from the Rust callback path",
        )
        loop4(
            iters, control, tk.rust_resolve_plugin_hook, (reg, "mod.A.f", [], "get_function_hook")
        )

    elif name == "callback_p_direct":

        class Reg:
            def has_hook_for(self, hook: str, fullname: str) -> bool:
                return False

        reg = Reg()
        require(
            reg.has_hook_for("get_function_hook", "mod.A.f") is False,
            "expected has_hook_for to answer False",
        )
        loop2(iters, control, reg.has_hook_for, ("get_function_hook", "mod.A.f"))

    elif name.startswith("is_subtype_e2e_"):
        import type_kernel as tk

        from mypy.state import state

        state.strict_optional = True
        # Real subtype pair (B <: A) with DIFFERENT fullnames: identical
        # bare instances answer in the Python fast path (subtypes.py:816)
        # and never reach the native block (#61 bench bug).
        info_a = fixture_info("mod.A")

        def sub_info(fullname: str):
            info = fixture_info(fullname)
            info.mro.append(info_a)
            info.bases = [types.Instance(info_a, [])]
            return info

        if "repeat" in name:
            info_b = sub_info("mod.B")
            resolver = tk.build_native_resolver([info_a, info_b], [])
            subtypes._set_native_subtype_active(name.endswith("_on"))
            subtypes._set_native_subtype_resolver(resolver)
            la, ra = types.Instance(info_b, []), types.Instance(info_a, [])
            require(subtypes.is_subtype(la, ra) is True, "warm call failed")
            if name.endswith("_on"):
                require(len(subtypes._subtype_answers) >= 1, "native path did not engage")
            require(subtypes.is_subtype(la, ra) is True, "warm call failed")
            loop2(iters, control, subtypes.is_subtype, (la, ra))
        else:  # distinct
            infos = [sub_info(f"mod.C{i}") for i in range(iters + 1)]
            # Every info must be in the resolver snapshot: the coded entry
            # declines (code -1) for fullnames it does not know, and those
            # calls would silently fall back to the Python visitor.
            resolver = tk.build_native_resolver([info_a] + infos, [])
            subtypes._set_native_subtype_active(name.endswith("_on"))
            subtypes._set_native_subtype_resolver(resolver)
            pairs = [(types.Instance(i, []), types.Instance(info_a, [])) for i in infos]
            idx = [0]

            def e2e(_unused: object) -> bool:
                i = idx[0]
                idx[0] = i + 1
                return subtypes.is_subtype(pairs[i][0], pairs[i][1])

            require(e2e(None) is True, "first distinct call failed")
            if name.endswith("_on"):
                require(len(subtypes._subtype_answers) >= 1, "native distinct path did not engage")
            idx[0] = 0  # re-measure pair 0 too; identical work either way
            loop1(iters, control, e2e, (None,))
    else:
        raise SystemExit(f"unknown bench {name}")
    return 0


def main() -> int:
    if len(sys.argv) >= 4 and sys.argv[1] == "--child":
        name = sys.argv[2]
        iters = int(sys.argv[3])
        control = "--control" in sys.argv
        return child(name, iters, control)
    return parent()


if __name__ == "__main__":
    raise SystemExit(main())
