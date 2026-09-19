#!/usr/bin/env python3
"""#1878 defer-arm instruction A/B harness for one type-kernel seam.

Wraps ONE seam binding in the probe process with a hard defer (a no-op the
Rust extension never sees), then runs mypy in-process: the defer arm is the
pure-Python path plus whatever Python-side gate/wire prep still runs before
the call. Under `/usr/bin/time -l` on the single-process cold self-check,
`delta = defer_arm - baseline` ranks the seam: negative means the crossing
costs instructions end to end (retire candidate), positive means keep.

Pinned invocation (run from the tree under test):

    PYTHONPATH=<scratch-ast>:<scratch-resolver>:<scratch-typekernel>:$PWD \
      MYPY_NUM_WORKERS=0 .venv/bin/python scripts/defer_seam_ab.py <seam> $PWD \
        --config-file mypy_self_check.ini -n0 --no-incremental \
        -p mypy -p mypyc --no-fast-exit

`<seam>` is the bare Rust name (e.g. `rust_is_subtype`); `-` is the default
arm (no patch, the baseline). Pre-flight: the editable finder and the cwd
sys.path entry are stripped and the requested tree is pinned first, so the
probe is immune to both foreign-tree channels (#1789); it refuses to run if
`mypy` resolves outside the pinned tree, and refuses unless
MYPY_NUM_WORKERS=0 so the self-check cannot fork unpatched workers.

Defer modes, per call-site contract:

  None     default: every host treats a None return as "fall back to Python".
  RAISE    seams whose call site consumes the return as a decided answer
           (`rust_has_type_vars`, `rust_is_subtype_coded`) get a wrapper that
           raises NotImplementedError instead: both sites already treat that
           exception as the defer signal, so a plain None would change
           semantics rather than defer.
  live     `rust_find_self_type` also patches `rust_find_self_type_live`
           because production dispatches to the live variant whenever the
           typeanal resolver is present.

Alias discovery is identity-based, not a hard-coded host list: every
`mypy.*`/`mypyc.*` submodule is imported and any module attribute whose
value `is` the original pyfunction is swapped for the defer, together with
the `type_kernel.rust_x` module attributes themselves. This reaches every
alias convention (`_rust_x`, plain rebinding, function-local imports) and
removes the risk of a silently partial patch biasing a delta toward zero;
hosts imported after the swap pick the defer up through
`from type_kernel import ...` themselves.
"""

from __future__ import annotations

import importlib
import os
import pkgutil
import sys
from collections.abc import Callable, Iterator
from types import ModuleType

# Seams whose production call site consumes the return value as a decided
# answer; a None no-op would corrupt semantics instead of deferring.
RAISE_SEAMS = frozenset({"rust_has_type_vars", "rust_is_subtype_coded"})

# Data/test subtrees never hold production seam aliases; importing them is
# pure cost in the probe.
SKIP_PREFIXES = ("mypy.test", "mypy.typeshed", "mypyc.test")


def _strip_foreign_import_channels() -> None:
    sys.meta_path[:] = [
        f
        for f in sys.meta_path
        if "editable" not in (f if isinstance(f, type) else type(f)).__module__.lower()
        and "editable" not in (f if isinstance(f, type) else type(f)).__name__.lower()
    ]
    cwd_entries = {"", os.getcwd(), os.path.realpath(os.getcwd())}
    sys.path[:] = [e for e in sys.path if e not in cwd_entries]


def _host_modules() -> Iterator[ModuleType]:
    for pkg_name in ("mypy", "mypyc"):
        pkg = importlib.import_module(pkg_name)
        yield pkg
        for info in pkgutil.walk_packages(getattr(pkg, "__path__", []), prefix=pkg_name + "."):
            if any(info.name.startswith(p) for p in SKIP_PREFIXES):
                continue
            try:
                yield importlib.import_module(info.name)
            except ImportError:
                continue


def patch_seam(name: str) -> str:
    """Patch every binding of `name` to a hard defer; return a patch report."""
    defer: Callable[..., object]
    if name in RAISE_SEAMS:

        def defer(*args: object, **kwargs: object) -> object:
            raise NotImplementedError("hard defer (defer_seam_ab)")

    else:

        def defer(*args: object, **kwargs: object) -> object:
            return None

    kernel = importlib.import_module("type_kernel")
    kernel_names = [name]
    if name == "rust_find_self_type":
        kernel_names.append("rust_find_self_type_live")
    originals: set[object] = set()
    for kernel_name in kernel_names:
        orig = getattr(kernel, kernel_name, None)
        if orig is not None:
            originals.add(orig)

    # Record alias bindings BEFORE the kernel attrs change: hosts imported
    # later pick the defer up via `from type_kernel import`, so the sweep
    # only needs already-imported modules; identity catches any convention.
    alias_hits: list[tuple[ModuleType, str]] = []
    for module in _host_modules():
        for attr, value in vars(module).items():
            if any(value is orig for orig in originals):
                alias_hits.append((module, attr))

    patched: list[str] = []
    for kernel_name in kernel_names:
        if getattr(kernel, kernel_name, None) is not None:
            setattr(kernel, kernel_name, defer)
            patched.append(f"type_kernel.{kernel_name}")
    for module, attr in alias_hits:
        setattr(module, attr, defer)
        patched.append(f"{module.__name__}.{attr}")

    return ", ".join(patched)


def main() -> None:
    if len(sys.argv) < 3:
        sys.exit("usage: defer_seam_ab.py <seam|-> <tree> [mypy args...]")
    if os.environ.get("MYPY_NUM_WORKERS", "") != "0":
        sys.exit("refusing: set MYPY_NUM_WORKERS=0 (the A/B pins single-process mode)")
    name = sys.argv[1]
    tree = os.path.realpath(sys.argv[2])
    sys.argv = ["mypy", *sys.argv[3:]]

    _strip_foreign_import_channels()
    sys.path.insert(0, tree)

    try:
        import mypy
    except ImportError:
        sys.exit(f"refusing: mypy not importable from the pinned tree: {tree}")

    pinned = os.path.join(tree, "mypy", "__init__.py")
    if os.path.realpath(mypy.__file__) != pinned:
        sys.exit(f"refusing: mypy resolved outside the pinned tree: {mypy.__file__}")

    if name != "-":
        report = patch_seam(name)
        print(f"[defer] {name}: {report or 'NOTHING PATCHED'}", file=sys.stderr)
        if not report:
            sys.exit(f"refusing: seam {name} has no binding to patch")
    else:
        print("[defer] none (default arm)", file=sys.stderr)

    from mypy.main import main

    main()


if __name__ == "__main__":
    main()
