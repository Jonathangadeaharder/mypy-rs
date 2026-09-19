#!/usr/bin/env python3
"""#70 blob-marshal falsifier: instruction-denominated per-call microbenches.

Bench lineage: `misc/gap_attrib_microbench.py` (#61). Every bench measures
one operation against a control loop of identical shape (a no-op Python
function with the same arity), so interpreter startup, mypy imports and
loop machinery cancel:

    per-call instructions = (instr(op run) - instr(control run)) / N

Instructions come from /usr/bin/time -l ("instructions retired"), which is
load-invariant. Each bench subprocess asserts, before the measured loop,
that the operation actually did its work (codes match the 12-arg coded
entry on a true pair and a false pair, the blob layout parses, counters
moved), so a bench cannot silently measure a no-op; the parent exits
non-zero if any engagement check fails. Every bench runs REPS rounds and
the table reports the per-op median plus the min/max spread.

Arms (issue #70 gate):
    coded12          current 12-arg `rust_is_subtype_coded` call
    blob_call        blob entry, prebuilt buffer (build excluded)
    blob_build_call  blob entry incl. one per-call Python buffer build
    blob_build       the Python-side buffer build alone (attribution)
    pyo3_1arg        bare 1-arg crossing, the control floor (#61)

Run from the worktree root (needs the scratch type_kernel .so):

    .venv/bin/python misc/blob_marshal_bench.py [--iters N] [--reps R]

Kernel benches use the in-repo extension via
/private/tmp/mypy-rs-local-typekernel (never the PyPI stub). Results print
as a table and land in /private/tmp/blob-marshal/microbench.txt.
"""

from __future__ import annotations

import os
import struct
import subprocess
import sys
import time
from collections.abc import Callable

# Worktree prelude (as in misc/gap_attrib_microbench.py): the shared .venv
# is an editable install whose finder maps `mypy` to a deleted worktree
# (#1789), and sys.path[0] is misc/. Pin this tree.
_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)
for _f in list(sys.meta_path):
    if "editable" in (type(_f).__module__ or "") or "editable" in repr(_f):
        sys.meta_path.remove(_f)

PY = sys.executable
HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = "/private/tmp/blob-marshal"
TK_SO = "/private/tmp/mypy-rs-local-typekernel/type_kernel.cpython-313-darwin.so"

# Blob wire layout, mirrored by `parse_subtype_blob_bench` in
# crates/type_kernel/src/subtypes.rs: u16 LE flag mask, u32 LE left
# length, left bytes, right bytes.
_BLOB_HDR = struct.Struct("<HI")

# (name, iters)
BENCHES = [
    ("coded12", 1_000_000),
    ("blob_call", 1_000_000),
    ("blob_build_call", 1_000_000),
    ("blob_build", 1_000_000),
    ("pyo3_1arg", 2_000_000),
]


def run_measured(child_args: list[str], env: dict[str, str]) -> int:
    proc = subprocess.run(
        ["/usr/bin/time", "-l", PY, os.path.join(HERE, "blob_marshal_bench.py")] + child_args,
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr[-4000:])
        raise SystemExit(f"bench subprocess failed: {child_args}")
    for line in proc.stderr.splitlines():
        if "instructions retired" in line:
            return int(line.split()[0].replace(",", "."))
    raise SystemExit(f"no instructions line for {child_args}: {proc.stderr[-2000:]}")


def parent() -> int:
    default_iters = None
    reps = 3
    if "--iters" in sys.argv:
        default_iters = int(sys.argv[sys.argv.index("--iters") + 1])
    if "--reps" in sys.argv:
        reps = int(sys.argv[sys.argv.index("--reps") + 1])
    if not os.path.exists(TK_SO):
        raise SystemExit(f"every bench needs the scratch type_kernel .so at {TK_SO}")
    os.makedirs(OUT_DIR, exist_ok=True)
    env = dict(os.environ)
    env["PYTHONPATH"] = "/private/tmp/mypy-rs-local-typekernel"
    env["MYPY_NUM_WORKERS"] = "0"
    env.setdefault("PYTHONDONTWRITEBYTECODE", "1")
    print(f"{'bench':16} {'iters':>9} {'median':>10} {'min':>10} {'max':>10}")
    rows = []
    t0 = time.time()
    for name, bench_iters in BENCHES:
        iters = default_iters or bench_iters
        per_call: list[float] = []
        for _ in range(reps):
            op_i = run_measured(["--child", name, str(iters)], env)
            ctl_i = run_measured(["--child", name, str(iters), "--control"], env)
            per_call.append((op_i - ctl_i) / iters)
        per_call.sort()
        med = per_call[len(per_call) // 2]
        print(f"{name:16} {iters:>9} {med:>10.1f} {per_call[0]:>10.1f} {per_call[-1]:>10.1f}")
        rows.append((name, iters, med, per_call[0], per_call[-1]))
    with open(os.path.join(OUT_DIR, "microbench.txt"), "w") as f:
        for name, iters, med, lo, hi in rows:
            f.write(f"{name}\t{iters}\t{med:.1f}\t{lo:.1f}\t{hi:.1f}\n")
    print(f"total wall {time.time() - t0:.0f}s; table in {OUT_DIR}/microbench.txt")
    return 0


def noop(*args: object) -> None:
    return None


# One measured-loop helper per call arity (as in gap_attrib_microbench.py):
# a shared tail would union every branch's op and arg at one `f(*a)` site,
# which static analysis misreads as an arity mismatch. Body identical.


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


def loop3(
    iters: int, control: bool, op: Callable[[object, object, object], object], a: tuple
) -> None:
    f = noop if control else op
    for _ in range(iters):
        f(*a)


def loop12(iters: int, control: bool, op: Callable[..., object], a: tuple) -> None:
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


def build_blob(lb: bytes, rb: bytes, mask: int) -> bytes:
    """Mirror of the production shim a blob variant would need: the two
    wire blobs (already built by `_serialize_type` in production, wire
    cache hit included) concatenated under one header plus the flag mask
    that the 12-arg form passes as eight booleans."""
    return _BLOB_HDR.pack(mask, len(lb)) + lb + rb


def kernel_fixtures():
    """The true/false pair, coded args and blobs every kernel bench shares.

    Engagement is cross-checked, not just asserted: the blob entry must
    answer exactly what the 12-arg coded entry answers on both pairs.
    """
    import type_kernel as tk

    from mypy import subtypes, types
    from mypy.state import state

    state.strict_optional = True
    info_a = fixture_info("mod.A")
    info_c = fixture_info("mod.C")
    resolver = tk.build_native_resolver([info_a, info_c], [])
    inst_a = types.Instance(info_a, [])
    inst_c = types.Instance(info_c, [])
    lb_a = subtypes._serialize_type(inst_a)
    lb_c = subtypes._serialize_type(inst_c)
    assert isinstance(lb_a, bytes) and lb_a, "serialization failed"
    # Same argument shape as the #61 pyo3_13args bench: identity pair,
    # every context flag False, infer_unions False -> mask 0.
    true_args = (
        lb_a,
        lb_a,
        False,
        False,
        False,
        False,
        False,
        False,
        False,
        False,
        resolver,
        False,
    )
    false_args = (
        lb_c,
        lb_a,
        False,
        False,
        False,
        False,
        False,
        False,
        False,
        False,
        resolver,
        False,
    )
    true_blob = build_blob(lb_a, lb_a, 0)
    false_blob = build_blob(lb_c, lb_a, 0)
    blob_entry = tk.rust_is_subtype_blob_bench
    assert tk.rust_is_subtype_coded(*true_args) == 1, "coded true pair not decided natively"
    assert tk.rust_is_subtype_coded(*false_args) == 0, "coded false pair not decided natively"
    assert blob_entry(true_blob, resolver) == 1, "blob true pair disagrees with coded entry"
    assert blob_entry(false_blob, resolver) == 0, "blob false pair disagrees with coded entry"
    assert blob_entry(b"\x00\x00", resolver) == -1, "short blob must defer (code -1)"
    assert true_blob == _BLOB_HDR.pack(0, len(lb_a)) + lb_a + lb_a, "blob layout drifted"
    return tk, resolver, true_args, true_blob, lb_a


def child(name: str, iters: int, control: bool) -> int:
    """One measured subprocess. Engagement-checked, then the bare loop runs."""
    if name == "pyo3_1arg":
        import type_kernel as tk

        before = tk.rust_checker_driver_counters()[0]
        tk.rust_checker_driver_record(0)
        after = tk.rust_checker_driver_counters()[0]
        assert after == before + 1, "counter did not move"
        loop1(iters, control, tk.rust_checker_driver_record, (0,))

    elif name == "coded12":
        tk, resolver, args, _, _ = kernel_fixtures()

        def coded12(*a: object) -> int:
            return tk.rust_is_subtype_coded(*a)  # type: ignore[attr-defined]

        assert coded12(*args) == 1, "crossing stopped answering"
        loop12(iters, control, coded12, args)

    elif name == "blob_call":
        tk, resolver, _, blob, _ = kernel_fixtures()
        blob_entry = tk.rust_is_subtype_blob_bench
        assert blob_entry(blob, resolver) == 1, "blob entry stopped answering"
        loop2(iters, control, blob_entry, (blob, resolver))

    elif name == "blob_build_call":
        tk, resolver, _, blob, lb = kernel_fixtures()
        blob_entry = tk.rust_is_subtype_blob_bench
        assert blob_entry(blob, resolver) == 1, "blob entry stopped answering"

        def build_call() -> int:
            return blob_entry(build_blob(lb, lb, 0), resolver)

        assert build_call() == 1, "build+call stopped answering"
        loop0(iters, control, build_call, ())

    elif name == "blob_build":
        from mypy import subtypes, types

        info = fixture_info("mod.A")
        inst = types.Instance(info, [])
        lb = subtypes._serialize_type(inst)
        assert isinstance(lb, bytes) and lb, "serialization failed"
        built = build_blob(lb, lb, 0)
        assert built == _BLOB_HDR.pack(0, len(lb)) + lb + lb, "blob layout drifted"
        assert len(built) == 6 + 2 * len(lb), "blob length wrong"
        loop3(iters, control, build_blob, (lb, lb, 0))

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
