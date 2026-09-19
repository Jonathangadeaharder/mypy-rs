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

Defer modes, per call-site contract. Every seam must be audited against
all of its call sites (plain and `_live`) before its first A/B and
added to the matching table, cross-checking the classifications in
scripts/measure_native_share.py; unclassified seams are refused, since
a wrong mode silently biases the delta instead of failing loudly:

  None     NONE_SEAMS (audited): every host treats a None return as
           "fall back to Python" (`if result is not None: ...` behind
           a defer-guard).
  RAISE    RAISE_SEAMS (audited): seams whose call site consumes the
           return as a decided answer, or whose success path is
           `try: _rust_x(...); return` where a returned None means
           *handled* (semanal_classprop full ports), get a wrapper
           raising NotImplementedError instead: those sites already
           treat that exception as the defer signal.
  refused  REFUSED_SEAMS (audited): consumed directly with no exception
           guard, no valid defer mode exists (a None would be returned
           as a value, a raise would crash the run).

Live-variant dispatch: several typeanal seams call `<seam>_live` (or
`_live_noresolver`) whenever the resolver is installed, which is the
normal state during a build, so an A/B on the plain name would leave the
exercised crossing unpatched. The harness therefore also patches the
`<seam>_live` and `<seam>_live_noresolver` kernel attributes whenever
they exist; those sites treat None as the defer signal.

Alias discovery is identity-based, not a hard-coded host list: every
`mypy.*`/`mypyc.*` submodule is imported and any module attribute whose
value `is` the original pyfunction is swapped for the defer, together with
the `type_kernel.rust_x` module attributes themselves. This reaches every
alias convention (`_rust_x`, plain rebinding, function-local imports) and
removes the risk of a silently partial patch biasing a delta toward zero;
hosts imported after the swap pick the defer up through
`from type_kernel import ...` themselves. Modules that fail to import are
listed in the patch report so a partial patch stays visible.
"""

from __future__ import annotations

import importlib
import os
import pkgutil
import sys
from collections.abc import Callable, Iterator
from types import ModuleType

# Seams whose call sites consume the return as a decided answer, or whose
# success path is `try: _rust_x(...); return` (returned None = handled
# PyOk unit); only the raise path defers. Mirrors measure_native_share.py.
RAISE_SEAMS = frozenset(
    {
        "rust_has_type_vars",
        "rust_is_subtype_coded",
        # semanal_classprop full ports (UNIT_HANDLED_SEAMS there).
        "rust_calculate_class_abstract_status",
        "rust_check_protocol_status",
        "rust_calculate_class_vars",
        "rust_add_type_promotion",
    }
)

# Seams whose call sites treat a None return as "fall back to Python",
# audited for the #1878 16-seam sweep and for the classifier-negative
# decisions (measure_native_share.py); _live sites were part of the audit.
NONE_SEAMS = frozenset(
    {
        "rust_freshen_function_type_vars",
        "rust_possible_none_type_var_overlap",
        "rust_get_declaration",
        "rust_find_self_type",
        "rust_check_overload_call",
        "rust_classify_simple_assignment",
        "rust_is_subtype",
        "rust_expand_type",
        "rust_custom_special_method",
        "rust_get_target_type",
        "rust_narrow_declared_type",
        "rust_infer_constraints_full",
        "rust_analyze_instance_member_dispatch",
        "rust_solve_generic_call",
        "rust_get_typevarlike_declaration",
        "rust_find_dataclass_transform_spec",
        "rust_find_duplicate",
        "rust_classify_protocol_test_callee",
    }
)

# Seams consumed directly with no exception guard: no defer mode exists.
# Audited via rg over mypy/ + mypyc/ for `return ...rust_x(...)` sites whose
# only guard is a gate flag; extend only after a call-site audit.
REFUSED_SEAMS = frozenset(
    {
        "rust_quote_type_string",
        "rust_variance_string",
        "rust_capitalize",
        "rust_extract_type",
        "rust_strip_quotes",
        "rust_format_item_name_list",
        "rust_wrong_type_arg_count",
        "rust_pretty_seq",
        "rust_best_matches",
        "rust_format_key_list",
        "rust_has_underscore_prefix",
        "rust_compute_search_paths",
        "rust_default_lib_path",
        "rust_get_search_dirs",
        "rust_is_init_file",
        "rust_load_stdlib_py_versions",
        "rust_matches_exclude",
        "rust_mypy_path",
        "rust_parse_version",
        "rust_typeshed_py_version",
    }
)

# Data/test subtrees never hold production seam aliases; importing them is
# pure cost in the probe.
SKIP_PREFIXES = ("mypy.test", "mypy.typeshed", "mypyc.test")


def _is_editable_finder(finder: object) -> bool:
    cls = finder if isinstance(finder, type) else type(finder)
    return "editable" in (cls.__module__ or "").lower() or "editable" in cls.__name__.lower()


def _strip_foreign_import_channels() -> None:
    sys.meta_path[:] = [f for f in sys.meta_path if not _is_editable_finder(f)]
    cwd_entries = {"", os.getcwd(), os.path.realpath(os.getcwd())}
    sys.path[:] = [e for e in sys.path if e not in cwd_entries]


def _host_modules(skipped: list[str]) -> Iterator[ModuleType]:
    for pkg_name in ("mypy", "mypyc"):
        pkg = importlib.import_module(pkg_name)
        yield pkg
        for info in pkgutil.walk_packages(getattr(pkg, "__path__", []), prefix=pkg_name + "."):
            if any(info.name.startswith(p) for p in SKIP_PREFIXES):
                continue
            try:
                yield importlib.import_module(info.name)
            except Exception:
                skipped.append(info.name)


def patch_seam(name: str) -> str:
    """Patch every binding of `name` to a hard defer; return a patch report."""
    if name in REFUSED_SEAMS:
        sys.exit(
            f"refusing: seam {name} is consumed directly with no defer "
            "contract (no valid None/raise defer mode); audit its call site "
            "and add an explicit classification before measuring it"
        )
    if name not in NONE_SEAMS and name not in RAISE_SEAMS:
        sys.exit(
            f"refusing: seam {name} has no audited defer classification; "
            "audit every call site (plain and _live) and add it to "
            "NONE_SEAMS, RAISE_SEAMS or REFUSED_SEAMS before measuring it"
        )
    defer: Callable[..., object]
    if name in RAISE_SEAMS:

        def defer(*args: object, **kwargs: object) -> object:
            raise NotImplementedError("hard defer (defer_seam_ab)")

    else:

        def defer(*args: object, **kwargs: object) -> object:
            return None

    kernel = importlib.import_module("type_kernel")
    # Live-variant dispatch: `<seam>_live` / `_live_noresolver` are the
    # paths actually executed while a resolver is installed, so they are
    # patched together with the plain name whenever they exist.
    kernel_names = [name]
    for suffix in ("_live", "_live_noresolver"):
        if hasattr(kernel, name + suffix):
            kernel_names.append(name + suffix)
    originals: set[object] = set()
    for kernel_name in kernel_names:
        orig = getattr(kernel, kernel_name, None)
        if orig is not None:
            originals.add(orig)

    # Record alias bindings BEFORE the kernel attrs change: hosts imported
    # later pick the defer up via `from type_kernel import`, so the sweep
    # only needs already-imported modules; identity catches any convention.
    alias_hits: list[tuple[ModuleType, str]] = []
    skipped_modules: list[str] = []
    for module in _host_modules(skipped_modules):
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

    report = ", ".join(patched)
    if skipped_modules:
        report += " | unimportable: " + ", ".join(sorted(skipped_modules))
    return report


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
        substantive, marker, unimportable = report.partition(" | unimportable: ")
        print(f"[defer] {name}: {substantive or 'NOTHING PATCHED'}", file=sys.stderr)
        if not substantive:
            sys.exit(f"refusing: seam {name} has no binding to patch")
        if marker:
            print(f"[defer] {name} WARNING: unimportable hosts: {unimportable}", file=sys.stderr)
    else:
        print("[defer] none (default arm)", file=sys.stderr)

    from mypy.main import main as mypy_main

    mypy_main()


if __name__ == "__main__":
    main()
