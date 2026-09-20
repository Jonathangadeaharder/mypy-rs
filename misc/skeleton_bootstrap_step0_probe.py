#!/usr/bin/env python3
"""#85 Step 0: skeleton-bootstrap checking-phase typeinfo read-closure probe.

Measures the #83 pre-registered falsifier quantity: the set of distinct
TypeInfo facts a checking phase consults. Runtime-only property wrappers
are installed over the read surfaces of ``mypy.nodes.TypeInfo`` fixed by
the brief (``names``, ``mro``, ``bases``) plus the member-lookup methods
of the kernel's consult vocabulary (``get``, ``get_method``,
``get_containing_type_info``). Nothing in mypy/ or crates/ changes; the
patches exist only while this driver runs.

Arming is phase-gated at checker start, the F3 wire-cache boundary
(``State.type_check_first_pass``, mypy/build.py:4801-4808): nothing is
recorded before the first checker entry. The counted closure is the
checker-window reading of the pre-registered "checking-phase
read-closure": reads are attributed to the module whose
``type_check_first_pass`` / ``type_check_second_pass`` frame is on the
stack, so per-SCC kernel snapshot re-feeds and later SCCs' semantic
analysis (both post-arming readers on this head) are excluded from the
counted population and reported only as the armed-closure diagnostic.

The corpus closure is read at family scope: the union of checker-window
closures over the corpus family's modules (the ``-p`` packages for the
self-check leg, the corpus module itself for the file corpora). Stdlib
stubs are checked only as import dependencies; the skeleton premise
ingests them as fixture data without checking them, so their windows
carry no corpus signal (the empty-control leg makes this measurable).

Guards, on every exit path: run failed or produced unexpected output,
arming boundary never crossed, zero checker-phase reads, empty closure,
corpus module never checked, id recycling among touched TypeInfos
(objects are pinned), native kernel engagement mismatch. The report is
always printed; exit 0 only on the probe's own evidence.

Counters are set sizes and counts, load-invariant; probe runs are never
instruction baselines (the #71/#81 probe-pin discipline).

Usage:
  .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus trivial
  .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus class-slice
  .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus selfcheck
  .venv/bin/python misc/skeleton_bootstrap_step0_probe.py --corpus empty-control
  --kernel-off disables the native type kernel (diagnostic leg only).
  Positional args are appended to the mypy argv (smoke runs only).
Prereq: PYTHONPATH with the type_kernel, ast_serialize and module_resolver
scratch dirs (AGENTS.md native build order) for the kernel-on legs.
Corpus class: /private/tmp/mypy-rs-sem.sh run 2 for the self-check leg,
run 1 for the file-corpus legs (trivial, class-slice, empty-control).
"""

from __future__ import annotations

import argparse
import io
import os
import sys

SELFCHECK_ARGS = [
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

TRIVIAL_MODULE_ID = "trivial_step0"

EMPTY_CONTROL_MODULE_ID = "empty_control"

EMPTY_CONTROL_TEXT = "pass\n"

TRIVIAL_TEXT = """\
x_int: int = 1
x_str: str = "text"
x_bool: bool = True
x_float: float = 2.5


def answer() -> int:
    return 42


x_bad: int = "not an int"
"""

# #114 grown corpus: one realistic class/member module plus the sibling it
# imports its base classes from. Inside the reader-fixed slice, outside the
# #93 hard-reject set. Mypy-clean, exit 0, two checked modules.
CLASS_SLICE_MODULE_ID = "class_slice"
CLASS_SLICE_BASE_MODULE_ID = "class_slice_base"

CLASS_SLICE_BASE_TEXT = """\
from typing import Generic, TypeVar

T = TypeVar("T")


class Shape:
    kind: str = "shape"

    def __init__(self, name: str) -> None:
        self.name = name

    def area(self) -> float:
        return 0.0

    def describe(self) -> str:
        return self.kind + ":" + self.name


class Sized(Generic[T]):
    def __init__(self, item: T) -> None:
        self.item = item

    def unwrap(self) -> T:
        return self.item

    def replace(self, item: T) -> "Sized[T]":
        return Sized(item)
"""

CLASS_SLICE_TEXT = """\
from typing import TypeVar

from class_slice_base import Shape, Sized

T = TypeVar("T")


class Circle(Shape):
    def __init__(self, radius: float) -> None:
        super().__init__("circle")
        self.radius = radius

    def area(self) -> float:
        return 3.14159 * self.radius * self.radius

    def describe(self) -> str:
        return super().describe() + "/r=" + str(self.radius)


class NamedBox(Sized[T]):
    def __init__(self, item: T, label: str) -> None:
        super().__init__(item)
        self.label = label

    def unwrap(self) -> T:
        return super().unwrap()


def combined_area(first: Shape, second: Shape) -> float:
    return first.area() + second.area()


def relabelled(box: Sized[int], label: str) -> NamedBox[int]:
    return NamedBox(box.unwrap(), label)


circle = Circle(2.0)
square = Shape("square")
area: float = combined_area(circle, square)
box = NamedBox[int](5, "five")
item: int = box.unwrap()
label: str = box.label
kind: str = Shape.kind
second: NamedBox[int] = relabelled(box, "six")
base_view: Shape = circle
view_area: float = base_view.area()
"""

SLOT_SURFACES = ("names", "mro", "bases")
LOOKUP_SURFACES = ("get", "get_method", "get_containing_type_info")
TAG = "[skel-step0]"

# Corpus family = the modules mypy was asked to check (-p packages or the
# corpus files). Only family windows count toward the corpus closure:
# stdlib stubs are fixture data for the skeleton, never checked by it.
FAMILY_PREFIXES = {
    "trivial": (TRIVIAL_MODULE_ID,),
    "empty-control": (EMPTY_CONTROL_MODULE_ID,),
    "class-slice": (CLASS_SLICE_MODULE_ID, CLASS_SLICE_BASE_MODULE_ID),
    "selfcheck": ("mypy", "mypyc"),
}


def make_probe() -> dict:
    return {
        "armed": False,
        "arm_count": 0,
        "first_pass_calls": 0,
        "second_pass_calls": 0,
        "window_stack": [],
        "window_modules": set(),
        "events": 0,
        "events_in_window": 0,
        "surface_events": {},
        "surface_events_in_window": {},
        "mod_fullnames": {},
        "mod_pairs": {},
        "armed_fullnames": set(),
        "armed_pairs": set(),
        "pinned": {},
        "id_recycled": 0,
    }


def fullname_of(info) -> str:
    try:
        return info.fullname or f"<anon@{id(info)}>"
    except Exception:
        return f"<anon@{id(info)}>"


def record(probe: dict, info, surface: str) -> None:
    probe["events"] += 1
    probe["surface_events"][surface] = probe["surface_events"].get(surface, 0) + 1
    key = id(info)
    pinned = probe["pinned"]
    prev = pinned.get(key)
    if prev is not None and prev is not info:
        probe["id_recycled"] += 1
    else:
        pinned[key] = info
    full = fullname_of(info)
    if probe["window_stack"]:
        probe["events_in_window"] += 1
        cur = probe["surface_events_in_window"]
        cur[surface] = cur.get(surface, 0) + 1
        mod = probe["window_stack"][-1]
        probe["mod_fullnames"].setdefault(mod, set()).add(full)
        probe["mod_pairs"].setdefault(mod, set()).add((full, surface))
    probe["armed_fullnames"].add(full)
    probe["armed_pairs"].add((full, surface))


def install_typeinfo_probe(probe: dict, nodes_mod) -> None:
    type_info = nodes_mod.TypeInfo

    for attr in SLOT_SURFACES:
        descr = type_info.__dict__[attr]
        orig_get = descr.__get__
        orig_set = descr.__set__

        def make_fget(attr=attr, orig_get=orig_get, cls=type_info):
            def fget(self):
                if probe["armed"]:
                    record(probe, self, attr)
                return orig_get(self, cls)

            return fget

        def make_fset(orig_set=orig_set):
            def fset(self, value):
                orig_set(self, value)

            return fset

        setattr(type_info, attr, property(make_fget(), make_fset()))

    for name in LOOKUP_SURFACES:
        orig = type_info.__dict__[name]

        def make_wrapper(name=name, orig=orig):
            def wrapper(self, member, *args, **kwargs):
                if probe["armed"]:
                    record(probe, self, f"member:{member}")
                return orig(self, member, *args, **kwargs)

            return wrapper

        setattr(type_info, name, make_wrapper())


def install_phase_probe(probe: dict, build_mod) -> None:
    step_cls = build_mod.State
    orig_first = step_cls.type_check_first_pass
    orig_second = step_cls.type_check_second_pass

    def first(self, recurse_into_functions: bool = True):
        probe["armed"] = True
        probe["arm_count"] += 1
        probe["first_pass_calls"] += 1
        probe["window_modules"].add(self.id)
        probe["window_stack"].append(self.id)
        try:
            return orig_first(self, recurse_into_functions)
        finally:
            probe["window_stack"].pop()

    def second(self, todo=None, recurse_into_functions: bool = True, impl_only: bool = False):
        probe["second_pass_calls"] += 1
        probe["window_modules"].add(self.id)
        probe["window_stack"].append(self.id)
        try:
            return orig_second(self, todo, recurse_into_functions, impl_only)
        finally:
            probe["window_stack"].pop()

    step_cls.type_check_first_pass = first
    step_cls.type_check_second_pass = second


def install_kernel_off(options_mod) -> None:
    orig_init = options_mod.Options.__init__

    def patched(self, *args, **kwargs):
        orig_init(self, *args, **kwargs)
        self.native_type_kernel = False

    options_mod.Options.__init__ = patched


def setup_trivial_corpus() -> list[str]:
    corpus_dir = os.path.join("/private/tmp", "skel-step0", "corpus")
    os.makedirs(corpus_dir, exist_ok=True)
    module_path = os.path.join(corpus_dir, TRIVIAL_MODULE_ID + ".py")
    with open(module_path, "w") as f:
        f.write(TRIVIAL_TEXT)
    config_path = os.path.join(corpus_dir, "trivial_mypy.ini")
    with open(config_path, "w") as f:
        f.write("[mypy]\n")
    return [
        "--config-file",
        config_path,
        "-n0",
        "--no-incremental",
        "--no-error-summary",
        module_path,
    ]


def setup_empty_control_corpus() -> list[str]:
    corpus_dir = os.path.join("/private/tmp", "skel-step0", "corpus")
    os.makedirs(corpus_dir, exist_ok=True)
    module_path = os.path.join(corpus_dir, EMPTY_CONTROL_MODULE_ID + ".py")
    with open(module_path, "w") as f:
        f.write(EMPTY_CONTROL_TEXT)
    config_path = os.path.join(corpus_dir, "empty_control_mypy.ini")
    with open(config_path, "w") as f:
        f.write("[mypy]\n")
    return ["--config-file", config_path, "-n0", "--no-incremental", module_path]


def setup_class_slice_corpus() -> list[str]:
    corpus_dir = os.path.join("/private/tmp", "skel-step0", "corpus")
    os.makedirs(corpus_dir, exist_ok=True)
    base_path = os.path.join(corpus_dir, CLASS_SLICE_BASE_MODULE_ID + ".py")
    with open(base_path, "w") as f:
        f.write(CLASS_SLICE_BASE_TEXT)
    module_path = os.path.join(corpus_dir, CLASS_SLICE_MODULE_ID + ".py")
    with open(module_path, "w") as f:
        f.write(CLASS_SLICE_TEXT)
    config_path = os.path.join(corpus_dir, "class_slice_mypy.ini")
    with open(config_path, "w") as f:
        f.write("[mypy]\n")
    return ["--config-file", config_path, "-n0", "--no-incremental", module_path]


def run_mypy(mypy_main, args: list[str]) -> tuple[str, str, int | None]:
    out = io.StringIO()
    run_status: str
    exit_code: int | None = None
    try:
        mypy_main.main(args=list(args), stdout=out, clean_exit=True)
        run_status = "completed, mypy returned without exit"
        exit_code = 0
    except SystemExit as exc:
        if exc.code in (0, None):
            run_status = "completed, mypy exit 0"
            exit_code = 0
        elif isinstance(exc.code, int):
            run_status = f"completed, mypy exit {exc.code}"
            exit_code = exc.code
        else:
            run_status = f"FAILED, mypy exit {exc.code!r}"
    except Exception as exc:
        run_status = f"FAILED, raised {type(exc).__name__}: {exc}"
        import traceback

        traceback.print_exc()
    return run_status, out.getvalue(), exit_code


def report(probe: dict, corpus: str, run_status: str, extra: dict) -> None:
    def p(line: str) -> None:
        print(f"{TAG} {line}", file=sys.stderr)

    p(f"corpus: {corpus}")
    p(f"run status: {run_status}")
    p(f"kernel-off mode: {extra['kernel_off']}")
    p(f"native resolver at end: {extra['resolver_set']}")
    for line in extra["evidence_lines"]:
        p(f"output evidence: {line}")
    p(
        f"arming: armed={probe['armed']}, arm_count={probe['arm_count']}, "
        f"first_pass_calls={probe['first_pass_calls']}, "
        f"second_pass_calls={probe['second_pass_calls']}"
    )
    p(f"read events: {probe['events']} total, {probe['events_in_window']} in window")
    p(f"surface events (total): {dict(sorted(probe['surface_events'].items()))}")
    p(f"surface events (in window): {dict(sorted(probe['surface_events_in_window'].items()))}")
    union_names: set = set()
    union_pairs: set = set()
    for names in probe["mod_fullnames"].values():
        union_names |= names
    for pairs in probe["mod_pairs"].values():
        union_pairs |= pairs
    p(
        f"checker-window closure (union over all checked modules): "
        f"{len(union_names)} fullnames, {len(union_pairs)} pairs"
    )
    p(
        f"armed closure (diagnostic, all post-arm readers): "
        f"{len(probe['armed_fullnames'])} fullnames, {len(probe['armed_pairs'])} pairs"
    )
    p(f"modules with checker windows: {len(probe['mod_fullnames'])}")
    p(f"modules entering checker windows: {len(probe['window_modules'])}")
    family_prefixes = FAMILY_PREFIXES[corpus]
    fam_names: set = set()
    fam_pairs: set = set()
    fam_modules = 0
    for mod in probe["window_modules"]:
        if any(mod == p or mod.startswith(p + ".") for p in family_prefixes):
            fam_modules += 1
            fam_names |= probe["mod_fullnames"].get(mod, set())
            fam_pairs |= probe["mod_pairs"].get(mod, set())
    p(
        f"corpus-family closure (union over checker windows of family modules): "
        f"{len(fam_names)} fullnames, {len(fam_pairs)} pairs"
    )
    p(f"corpus-family modules entering checker windows: {fam_modules}")
    pkg_counts: dict = {}
    for name in fam_names:
        top = name.split(".")[0]
        pkg_counts[top] = pkg_counts.get(top, 0) + 1
    p(f"corpus-family closure by top-level package: {dict(sorted(pkg_counts.items()))}")
    p(f"touched infos pinned: {len(probe['pinned'])}, id_recycled={probe['id_recycled']}")
    for mod in sorted(probe["mod_fullnames"]):
        p(
            f"module {mod!r}: closure {len(probe['mod_fullnames'][mod])} fullnames, "
            f"{len(probe['mod_pairs'][mod])} pairs"
        )
    if extra["corpus_module_id"] in probe["window_modules"]:
        names = probe["mod_fullnames"].get(extra["corpus_module_id"], set())
        pairs = probe["mod_pairs"].get(extra["corpus_module_id"], set())
        p(
            f"corpus module {extra['corpus_module_id']!r} closure: "
            f"{len(names)} fullnames, {len(pairs)} pairs"
        )
        if names:
            p("corpus module closure fullnames (sorted):")
            for name in sorted(names):
                p(f"  {name}")
            p("corpus module pairs (sorted):")
            for pair in sorted(pairs):
                p(f"  {pair[0]} :: {pair[1]}")


def evidence_refusals(probe: dict, run_status: str, output: str, extra: dict) -> list[str]:
    reasons = []
    if not run_status.startswith("completed"):
        reasons.append(f"run status {run_status!r} is not a completed check")
    if "INTERNAL ERROR" in output or "INTERNAL ERROR" in run_status:
        reasons.append("mypy reported an INTERNAL ERROR")
    reasons.extend(extra["expectation_failures"])
    if probe["arm_count"] == 0:
        reasons.append("arming boundary never crossed: no type_check_first_pass call")
    if probe["events_in_window"] == 0:
        reasons.append("zero checker-phase reads: nothing was measured")
    if not probe["mod_fullnames"]:
        reasons.append("checker-window closure empty: nothing was measured")
    for module_id in extra["required_module_ids"]:
        if module_id not in probe["window_modules"]:
            reasons.append(f"corpus module {module_id!r} never entered a checker window")
    if probe["id_recycled"]:
        reasons.append(
            f"{probe['id_recycled']} id recyclings among touched infos: "
            "closure identity is not trustworthy"
        )
    if extra["kernel_off"] and extra["resolver_set"]:
        reasons.append("kernel-off mode requested but the native resolver engaged")
    if not extra["kernel_off"] and not extra["resolver_set"]:
        reasons.append(
            "native kernel never engaged (scratch type_kernel on PYTHONPATH, "
            "AGENTS.md native build order)"
        )
    return reasons


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--corpus",
        choices=("trivial", "class-slice", "selfcheck", "empty-control"),
        default="selfcheck",
    )
    parser.add_argument(
        "--kernel-off",
        action="store_true",
        help="diagnostic leg: disable Options.native_type_kernel",
    )
    parser.add_argument("mypy_extra", nargs="*", help=argparse.SUPPRESS)
    opts = parser.parse_args()

    # Worktree prelude (same as misc/arena_amortization_step0_probe.py): the
    # shared .venv's editable finder maps `mypy` elsewhere (#1789), and
    # sys.path[0] is misc/, not the root. Pin this tree.
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if root not in sys.path:
        sys.path.insert(0, root)
    for finder in list(sys.meta_path):
        if "editable" in (type(finder).__module__ or "") or "editable" in repr(finder):
            sys.meta_path.remove(finder)
    os.environ["MYPY_NUM_WORKERS"] = "0"
    import mypy

    if not os.path.abspath(mypy.__file__).startswith(root + os.sep):
        print(f"{TAG} mypy resolves outside this tree: {mypy.__file__}", file=sys.stderr)
        return 1
    import mypy.build
    import mypy.nodes
    import mypy.options
    import mypy.subtypes

    probe = make_probe()
    install_typeinfo_probe(probe, mypy.nodes)
    install_phase_probe(probe, mypy.build)
    if opts.kernel_off:
        install_kernel_off(mypy.options)

    if opts.corpus == "trivial":
        args = setup_trivial_corpus()
        corpus_module_id = TRIVIAL_MODULE_ID
        required_module_ids = (TRIVIAL_MODULE_ID,)
    elif opts.corpus == "empty-control":
        args = setup_empty_control_corpus()
        corpus_module_id = EMPTY_CONTROL_MODULE_ID
        required_module_ids = (EMPTY_CONTROL_MODULE_ID,)
    elif opts.corpus == "class-slice":
        args = setup_class_slice_corpus()
        corpus_module_id = CLASS_SLICE_MODULE_ID
        required_module_ids = (CLASS_SLICE_MODULE_ID, CLASS_SLICE_BASE_MODULE_ID)
    else:
        args = list(SELFCHECK_ARGS)
        corpus_module_id = "mypy"
        required_module_ids = ("mypy", "mypyc")
    args.extend(opts.mypy_extra)

    import mypy.main

    try:
        run_status, output, exit_code = run_mypy(mypy.main, args)
    except KeyboardInterrupt:
        extra = {
            "kernel_off": opts.kernel_off,
            "resolver_set": None,
            "evidence_lines": [],
            "corpus_module_id": corpus_module_id,
            "required_module_ids": required_module_ids,
            "expectation_failures": ["run interrupted"],
        }
        report(probe, opts.corpus, "INTERRUPTED, operator Ctrl-C", extra)
        print(f"{TAG} EVIDENCE REFUSED: run was interrupted", file=sys.stderr)
        raise
    resolver_set = mypy.subtypes._native_subtype_resolver is not None
    evidence_lines: list[str] = []
    expectation_failures: list[str] = []
    if opts.corpus == "trivial":
        hits = output.count("Incompatible types in assignment")
        evidence_lines.append(f"deliberate-error occurrences: {hits}")
        if run_status.startswith("completed"):
            if exit_code != 1:
                expectation_failures.append(
                    f"trivial corpus must exit 1 (one deliberate error), got {exit_code}"
                )
            if hits != 1:
                expectation_failures.append(
                    f"expected exactly 1 'Incompatible types in assignment', got {hits}"
                )
    elif opts.corpus == "empty-control":
        success = [ln for ln in output.splitlines() if ln.startswith("Success:")]
        evidence_lines.append(f"success line: {success[0] if success else '<missing>'}")
        if run_status.startswith("completed"):
            if exit_code != 0:
                expectation_failures.append(f"empty control must exit 0, got {exit_code}")
            if not success:
                expectation_failures.append("empty control success line missing")
    elif opts.corpus == "class-slice":
        success = [ln for ln in output.splitlines() if ln.startswith("Success:")]
        evidence_lines.append(f"success line: {success[0] if success else '<missing>'}")
        if run_status.startswith("completed"):
            if exit_code != 0:
                expectation_failures.append(f"class-slice corpus must exit 0, got {exit_code}")
            if not success:
                expectation_failures.append("class-slice success line missing")
    else:
        success = [ln for ln in output.splitlines() if ln.startswith("Success:")]
        evidence_lines.append(f"success line: {success[0] if success else '<missing>'}")
        if run_status.startswith("completed"):
            if exit_code != 0:
                expectation_failures.append(f"self-check must exit 0, got {exit_code}")
            if not success:
                expectation_failures.append("self-check success line missing")

    extra = {
        "kernel_off": opts.kernel_off,
        "resolver_set": resolver_set,
        "evidence_lines": evidence_lines,
        "corpus_module_id": corpus_module_id,
        "required_module_ids": required_module_ids,
        "expectation_failures": expectation_failures,
    }
    report(probe, opts.corpus, run_status, extra)
    refusals = evidence_refusals(probe, run_status, output, extra)
    for reason in refusals:
        print(f"{TAG} EVIDENCE REFUSED: {reason}", file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main())
