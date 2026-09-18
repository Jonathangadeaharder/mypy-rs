#!/usr/bin/env python3
"""#1878 wire-prep measurement: Python-side prep time per kernel seam.

Measures the cost a defer A/B cannot see (#1878): the Python work that
builds wire bytes as arguments BEFORE each `_type_kernel.rust_*` call
(`_serialize_type(...)`, `_serialize_env(...)`, buffer writes), on a
single-process cold self-check. Also splits that prep by call outcome
(useful / deferred / raised), so a cheap Python pre-gate candidate list
can be ranked by the prep it would actually remove.

Pinned invocation (run from the worktree root):

    PYTHONPATH=/private/tmp/mypy-rs-local-typekernel:\
/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver \
      .venv/bin/python scripts/measure_wire_prep.py

The argv, the fan-out refusal, the kernel guard and the run-outcome
classification are reused from `misc/audit_wire_traffic.py`; `-n0` is
load-bearing (#1820) because the counters are in-process.

Measurement model (every prep event lands in exactly one bucket):

  ledger S  serialize entries: the outermost `_serialize*` call per prep
            (nested `_serialize*` calls are subsumed by the outer one).
            The entry's duration is keyed on the identity of the returned
            bytes and handed to the seam that consumes those bytes; an
            unconsumed entry (e.g. a `_subtype_answers` dedup key) is
            reported as such, counted per pending blob (a multi-blob
            result contributes several, only the first carrying time),
            so unconsumed counts can exceed the entry count. A wire-cache
            hit that re-serves a blob re-accumulates onto the same key,
            so the next consuming seam pays the hit's cost, matching who
            actually spent it. A multi-blob result (a
            `_serialize_type_list*`) is one entry: the first blob carries
            the duration, the rest carry 0, so consuming two of them
            cannot double-count. An entry that raises (PlaceholderType
            and friends: callers catch and defer to Python) stays in the
            raised bucket, never attributed to a seam.
  ledger W  adapter windows: the buffer-built seams (constraints, solve,
            argmap...) serialize via raw `t.write(buf)` + `buf.getvalue()`
            rather than `_serialize*` funnels, so the named adapter
            functions are wrapped and the window from adapter entry to
            the adapter's target seam call is timed. Windows reset at
            every wrapped seam call, so gate work that itself calls other
            seams is discarded, never misattributed as prep. The window
            residual - time that is neither a timed serialize entry, nor
            a seam call, nor an attributed window - is raw buffer prep,
            gate checks and argument glue; it is split by whether the
            adapter made any seam call, so prep for calls the adapter
            never made is visible, not buried.

Known-answer validation runs first (`--self-test`): a forced seam-call
count, a wire-cache accumulation case, an unconsumed case, an adapter
window case, an alias-seam case, a kwarg-nesting consumption case and a
patch-idempotence case, each asserted exactly, plus a global conservation
identity (seam-attributed + unconsumed + no-blob + raised serialize time
== total serialize entry time) and a wrapper-overhead calibration check.
The probe exits non-zero if any check fails, if the audited run fails, or
if the coverage census finds a loaded-but-unpatched `_serialize*` helper
(an undercount is worse than no measurement). Numbers are nanosecond
integers, so the identity is exact, not approximate.

`--calibrate` measures the instrumentation itself: the serialize wrapper
costs more per event than the raw serializer (timing, frame lookup, site
string, blob pinning). It prints both rates so the wrapper overhead is
reproducible per machine; subtract it when quoting absolute prep numbers.
"""

from __future__ import annotations

import os as _os0
import sys as _sys0

_ROOT = _os0.path.dirname(_os0.path.dirname(_os0.path.abspath(__file__)))
if _ROOT not in _sys0.path:
    _sys0.path.insert(0, _ROOT)
# Pin `mypy` to this tree before anything else imports it: the shared
# .venv is a PEP 660 editable install whose finder resolves `mypy` to
# whichever tree installed into it last (#1789).
_os0.environ.setdefault("MYPY_AUDIT_ROOT", _ROOT)

import collections
import os
import sys
import time
import types as _types_mod
from collections.abc import Callable
from typing import Any

import misc.audit_wire_traffic as audit

# Defer classification (source of truth: the audit tool, so both tools
# agree on which None results are deferrals and which are decisions).
CLASSIFIER_NEGATIVE = audit.CLASSIFIER_NEGATIVE_SEAMS
UNIT_HANDLED = audit.UNIT_HANDLED_SEAMS
BATCH_SLOT = audit._BATCH_SLOT_SEAMS
CODED_DECISION = audit._CODED_DECISION_SEAMS

_perf = time.perf_counter_ns

# Serialized modules imported before patching so their helpers cannot
# escape the patch by loading late (plugins load during the build).
SERIALIZER_MODULES: tuple[str, ...] = (
    "mypy.types",
    "mypy.subtypes",
    "mypy.constraints",
    "mypy.solve",
    "mypy.expandtype",
    "mypy.join",
    "mypy.meet",
    "mypy.checkmember",
    "mypy.checkpattern",
    "mypy.checkexpr",
    "mypy.checker",
    "mypy.typeops",
    "mypy.erasetype",
    "mypy.applytype",
    "mypy.maptype",
    "mypy.typevars",
    "mypy.messages",
    "mypy.traverser",
    "mypy.typeanal",
    "mypy.semanal",
    "mypy.argmap",
    "mypy.copytype",
    "mypy.plugins",
    "mypy.plugins.attrs",
    "mypy.plugins.dataclasses",
)

# Buffer-built adapters: (module, function-or-Class.method, target seam).
# These build wire bytes via raw `Type.write(buf)` + `buf.getvalue()`, so
# ledger S cannot see them; `_serialize*` funnels must NOT appear here.
ADAPTERS: tuple[tuple[str, str, str], ...] = (
    ("mypy.constraints", "_try_native_infer_constraints", "rust_infer_constraints"),
    ("mypy.constraints", "_try_native_constraint_builder", "rust_infer_constraints_full"),
    ("mypy.constraints", "_try_native_select_trivial", "rust_select_trivial"),
    ("mypy.constraints", "_try_native_exclude_non_meta_vars", "rust_exclude_non_meta_vars"),
    ("mypy.constraints", "_try_native_is_similar_constraints", "rust_is_similar_constraints"),
    ("mypy.constraints", "_try_native_merge_with_any", "rust_merge_with_any"),
    ("mypy.constraints", "_try_native_filter_satisfiable", "rust_filter_satisfiable"),
    ("mypy.constraints", "_try_native_is_same_constraints", "rust_is_same_constraints"),
    ("mypy.constraints", "_try_native_any_constraints", "rust_any_constraints"),
    ("mypy.constraints", "_try_native_repack_callable_args", "rust_repack_callable_args"),
    ("mypy.constraints", "_try_native_filter_imprecise_kinds", "rust_filter_imprecise_kinds"),
    ("mypy.constraints", "_try_native_is_type_type", "rust_is_type_type"),
    ("mypy.constraints", "_try_native_unwrap_type_type", "rust_unwrap_type_type"),
    (
        "mypy.constraints",
        "_try_native_find_matching_overload_items",
        "rust_find_matching_overload_items",
    ),
    (
        "mypy.constraints",
        "_try_native_infer_directed_arg_constraints",
        "rust_infer_directed_arg_constraints",
    ),
    (
        "mypy.constraints",
        "_try_native_infer_callable_args",
        "rust_infer_callable_arguments_constraints",
    ),
    ("mypy.solve", "skip_reverse_union_constraints", "rust_skip_reverse_union_constraints"),
    ("mypy.expandtype", "remove_trivial", "rust_remove_trivial"),
    ("mypy.maptype", "_native_map_step_frontier", "rust_map_instance_to_supertypes"),
    ("mypy.typevars", "has_no_typevars", "rust_has_no_typevars"),
    ("mypy.checkmember", "add_class_tvars", "rust_add_class_tvars"),
    ("mypy.copytype", "copy_type", "rust_copy_type"),
    ("mypy.argmap", "ArgTypeExpander.expand_actual_type", "rust_expand_actual_type"),
    ("mypy.checker", "_try_native_stmt_outcome", "rust_stmt_outcome"),
)

# --- ledger state ------------------------------------------------------------
# Originals captured at patch time so calibration can time the raw
# serializer against the wrapped one in the same process.
_orig_serializers: dict[str, Callable[..., Any]] = {}

_serialize_depth = 0

serialize_entry_ns = 0
serialize_entry_events = 0
site_ns: collections.Counter[str] = collections.Counter()
site_events: collections.Counter[str] = collections.Counter()

# Serialize attempts that raised before producing a blob: real prep
# time on a native attempt that never reached a seam. Raising is a
# production path: callers catch and defer to Python (#1878).
serialize_raised_ns = 0
serialize_raised_events = 0
raised_site_ns: collections.Counter[str] = collections.Counter()

# id(blob) -> [accumulated_ns, site]; pins keep ids stable while pending.
pending: dict[int, list[Any]] = {}
_pins: dict[int, Any] = {}
_pinned_bytes = 0
_TRACKED_WARN_BYTES = 1 << 30
_pinned_warned = False

# Unconsumed counts are per pending BLOB, not per serialize entry: a
# multi-blob result contributes several blobs (only the first carries
# time), so `unconsumed_events` can exceed the serialize entry count.
unconsumed_ns = 0
unconsumed_events = 0
unconsumed_site_ns: collections.Counter[str] = collections.Counter()
no_blob_ns = 0
no_blob_events = 0

# Per-seam counters (keys are normalized seam names, no leading "_").
seam_calls: collections.Counter[str] = collections.Counter()
seam_defers: collections.Counter[str] = collections.Counter()
seam_call_ns: collections.Counter[str] = collections.Counter()
prep_useful_ns: collections.Counter[str] = collections.Counter()
prep_deferred_ns: collections.Counter[str] = collections.Counter()
prep_raised_ns: collections.Counter[str] = collections.Counter()
prep_events: collections.Counter[str] = collections.Counter()
prep_nonzero_events: collections.Counter[str] = collections.Counter()
window_useful_ns: collections.Counter[str] = collections.Counter()
window_deferred_ns: collections.Counter[str] = collections.Counter()
window_raised_ns: collections.Counter[str] = collections.Counter()
window_events: collections.Counter[str] = collections.Counter()
window_unattributed_ns = 0
# Adapter residual: window time no ledger sees (raw t.write buffer
# prep, gate checks, argument glue), split by whether the adapter made
# any seam call so prep for never-made calls is visible, not buried.
adapter_residual_called_ns = 0
adapter_residual_uncalled_ns = 0
registered_seams: set[str] = set()
alias_seams: set[str] = set()

# Adapter window stack of `_AdapterFrame` (t0, sub, attributed, target).
_stack: list[_AdapterFrame] = []


def _site_of(frame: Any) -> str:
    code = frame.f_code
    return f"{os.path.basename(code.co_filename)}:{frame.f_lineno}:{code.co_name}"


def _activity_done(dt: int) -> None:
    """Charge a timed activity to the enclosing adapter window, if any."""
    if _stack and dt:
        _stack[-1].sub += dt


class _AdapterFrame:
    __slots__ = ("t0", "sub", "attributed", "target", "seam_called")

    def __init__(self, target: str) -> None:
        self.t0 = _perf()
        self.sub = 0
        self.attributed = 0
        self.target = target
        self.seam_called = False


def _wrap_adapter(fn: Callable[..., Any], target: str) -> Callable[..., Any]:
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        frame = _AdapterFrame(target)
        _stack.append(frame)
        try:
            return fn(*args, **kwargs)
        finally:
            _stack.pop()
            total = _perf() - frame.t0
            residual = total - frame.sub - frame.attributed
            if residual > 0:
                global adapter_residual_called_ns, adapter_residual_uncalled_ns
                if frame.seam_called:
                    adapter_residual_called_ns += residual
                else:
                    adapter_residual_uncalled_ns += residual
            _activity_done(total)

    wrapper.__name__ = getattr(fn, "__name__", target)
    wrapper._wire_prep_patched = True
    return wrapper


def _take_window(seam: str) -> int:
    """Attribute the current adapter window to `seam` if it is the target.

    Every wrapped seam call resets the top window, so gate work that
    calls other seams first is discarded, never counted as prep.
    """
    if not _stack:
        return 0
    top = _stack[-1]
    ns = _perf() - top.t0 - top.sub
    hit = None
    for frame in reversed(_stack):
        if frame.target == seam:
            hit = frame
            break
    if hit is not None and ns > 0:
        hit.attributed += ns
        result = ns
    else:
        global window_unattributed_ns
        if ns > 0:
            window_unattributed_ns += ns
        result = 0
    top.t0 = _perf()
    top.sub = 0
    return result


def _blobs_of(value: Any) -> list[Any]:
    if isinstance(value, (bytes, bytearray, memoryview)):
        return [value]
    if isinstance(value, (list, tuple)):
        return [b for b in value if isinstance(b, (bytes, bytearray, memoryview))]
    return []


def _iter_arg_blobs(args: tuple[Any, ...], kwargs: dict[str, Any]) -> list[Any]:
    """Blobs reachable from call arguments, positional and keyword alike."""
    out: list[Any] = []
    for a in list(args) + list(kwargs.values()):
        out.extend(_blobs_of(a))
        if isinstance(a, (list, tuple)):
            for x in a:
                out.extend(_blobs_of(x))
    return out


def _record_entry(result: Any, dt: int) -> None:
    global serialize_entry_ns, serialize_entry_events
    global no_blob_ns, no_blob_events, _pinned_bytes, _pinned_warned
    serialize_entry_ns += dt
    serialize_entry_events += 1
    frame = sys._getframe(2)
    site = _site_of(frame)
    site_ns[site] += dt
    site_events[site] += 1
    blobs = _blobs_of(result)
    if not blobs:
        no_blob_ns += dt
        no_blob_events += 1
        return
    # A multi-blob result (a `_serialize_type_list*`) is ONE entry with
    # one duration: charging dt to every blob double-counts the moment
    # two of them reach seams. The first blob carries it, the rest 0.
    for i, b in enumerate(blobs):
        key = id(b)
        blob_dt = dt if i == 0 else 0
        entry = pending.get(key)
        if entry is not None:
            if blob_dt:
                entry[0] += blob_dt
                entry[1] = site
        else:
            pending[key] = [blob_dt, site]
            _pins[key] = b
            _pinned_bytes += len(b)
            if not _pinned_warned and _pinned_bytes >= _TRACKED_WARN_BYTES:
                _pinned_warned = True
                print(
                    f"measure_wire_prep: pinned blob bytes passed "
                    f"{_TRACKED_WARN_BYTES // (1 << 20)} MiB; unconsumed blobs are "
                    f"pinned (kept alive) until finalize, so memory keeps growing",
                    file=sys.stderr,
                )
    _activity_done(dt)


def _record_raised(dt: int) -> None:
    """Count a serialize attempt that raised before producing a blob."""
    global serialize_entry_ns, serialize_entry_events
    global serialize_raised_ns, serialize_raised_events
    serialize_entry_ns += dt
    serialize_entry_events += 1
    serialize_raised_ns += dt
    serialize_raised_events += 1
    raised_site_ns[_site_of(sys._getframe(2))] += dt
    _activity_done(dt)


def _wrap_serializer(name: str, fn: Callable[..., Any]) -> Callable[..., Any]:
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        global _serialize_depth
        if _serialize_depth:
            # Nested `_serialize*` call: subsumed by the outer entry.
            return fn(*args, **kwargs)
        _serialize_depth += 1
        t0 = _perf()
        try:
            result = fn(*args, **kwargs)
        except BaseException:
            _serialize_depth -= 1
            _record_raised(_perf() - t0)
            raise
        _serialize_depth -= 1
        _record_entry(result, _perf() - t0)
        return result

    wrapper.__name__ = name
    wrapper._wire_prep_patched = True
    return wrapper


def _consume_args(args: tuple[Any, ...], kwargs: dict[str, Any], seam: str, outcome: str) -> None:
    for b in _iter_arg_blobs(args, kwargs):
        key = id(b)
        entry = pending.pop(key, None)
        if entry is None:
            continue
        _pins.pop(key, None)
        dt = entry[0]
        prep_events[seam] += 1
        if dt:
            prep_nonzero_events[seam] += 1
        if outcome == "useful":
            prep_useful_ns[seam] += dt
        elif outcome == "deferred":
            prep_deferred_ns[seam] += dt
        else:
            prep_raised_ns[seam] += dt


def _deferred_result(seam: str, result: Any) -> bool:
    if seam in BATCH_SLOT:
        return isinstance(result, list) and any(s == -1 for s in result)
    if seam in CODED_DECISION:
        return result == -1
    if result is None and seam not in CLASSIFIER_NEGATIVE:
        if seam not in UNIT_HANDLED:
            return True
    return False


def _wrap_seam(name: str, fn: Callable[..., Any], alias: bool) -> Callable[..., Any]:
    registered_seams.add(name)
    if alias:
        alias_seams.add(name)

    def wrapper(*args: Any, **kwargs: Any) -> Any:
        seam_calls[name] += 1
        if _stack:
            for frame in _stack:
                frame.seam_called = True
        window_ns = _take_window(name)
        timed = bool(_stack)
        t0 = _perf() if timed else 0
        try:
            result = fn(*args, **kwargs)
        except BaseException:
            if timed:
                dt = _perf() - t0
                _activity_done(dt)
                seam_call_ns[name] += dt
            _consume_args(args, kwargs, name, "raised")
            if window_ns:
                window_events[name] += 1
                window_raised_ns[name] += window_ns
            raise
        if timed:
            dt = _perf() - t0
            _activity_done(dt)
            seam_call_ns[name] += dt
        deferred = _deferred_result(name, result)
        if deferred:
            seam_defers[name] += 1
        _consume_args(args, kwargs, name, "deferred" if deferred else "useful")
        if window_ns:
            window_events[name] += 1
            if deferred:
                window_deferred_ns[name] += window_ns
            else:
                window_useful_ns[name] += window_ns
        return result

    wrapper.__name__ = name
    wrapper._wire_prep_patched = True
    return wrapper


# --- patching ----------------------------------------------------------------


def _import_all() -> None:
    import importlib

    failed = []
    for modname in SERIALIZER_MODULES:
        try:
            importlib.import_module(modname)
        except ImportError:
            failed.append(modname)
    if failed:
        raise RuntimeError(f"serializer modules failed to import: {failed}")


def patch_serializers() -> int:
    n = 0
    for modname, mod in list(sys.modules.items()):
        if not modname.startswith("mypy.") or mod is None:
            continue
        for name in dir(mod):
            if not name.startswith("_serialize"):
                continue
            attr = getattr(mod, name)
            if not isinstance(attr, _types_mod.FunctionType):
                continue
            if getattr(attr, "_wire_prep_patched", False):
                continue
            _orig_serializers[f"{modname}.{name}"] = attr
            setattr(mod, name, _wrap_serializer(name, attr))
            n += 1
    return n


def _restore_serializers() -> None:
    """Undo patch_serializers, for calibration only.

    The raw baseline must run on the truly uninstrumented tree: with
    wrappers live, a directly-called `_serialize_type` leaves the depth
    gauge at 0, so every nested `_serialize*` helper pays the full
    wrapper entry cost that uninstrumented production code never pays.
    """
    for key, fn in _orig_serializers.items():
        modname, name = key.rsplit(".", 1)
        mod = sys.modules.get(modname)
        if mod is not None:
            setattr(mod, name, fn)


def patch_seams() -> int:
    try:
        import type_kernel
    except ImportError as exc:
        print(audit.missing_kernel_reason(exc), file=sys.stderr)
        raise audit.KernelUnavailableError() from exc
    n = 0
    for name in dir(type_kernel):
        if not name.startswith("rust_") or not callable(getattr(type_kernel, name)):
            continue
        fn = getattr(type_kernel, name)
        if getattr(fn, "_wire_prep_patched", False):
            continue
        setattr(type_kernel, name, _wrap_seam(name, fn, False))
        n += 1
    for modname, mod in list(sys.modules.items()):
        if not modname.startswith("mypy.") or mod is None:
            continue
        for name in dir(mod):
            if not name.startswith("_rust_"):
                continue
            attr = getattr(mod, name)
            if not callable(attr) or getattr(attr, "_wire_prep_patched", False):
                continue
            setattr(mod, name, _wrap_seam(name[1:], attr, True))
            n += 1
    return n


def patch_adapters() -> int:
    import importlib

    problems = []
    for modname, path, target in ADAPTERS:
        mod = importlib.import_module(modname)
        if "." in path:
            cls_name, method = path.split(".")
            target_obj: Any = getattr(mod, cls_name)
            orig = getattr(target_obj, method)
            holder: Any = target_obj
            attr = method
        else:
            orig = getattr(mod, path)
            holder = mod
            attr = path
        if not callable(orig):
            problems.append(f"{modname}.{path} is not callable")
            continue
        if getattr(orig, "_wire_prep_patched", False):
            continue
        setattr(holder, attr, _wrap_adapter(orig, target))
    if problems:
        raise RuntimeError(f"adapter enumeration is stale: {problems}")
    missing = [t for _, _, t in ADAPTERS if t not in registered_seams]
    if missing:
        raise RuntimeError(f"adapter target seams were never registered: {missing}")
    return len(ADAPTERS)


def coverage_census() -> list[str]:
    """Loaded-but-unpatched helpers: a silent undercount source.

    `_rust_` names must be wrapped seams; a bare `rust_` binding in a
    mypy module would be an import the alias pass missed (#1878 has no
    such binding today; this guard makes drift loud instead of silent).
    """
    gaps = []
    for modname, mod in list(sys.modules.items()):
        if not modname.startswith("mypy.") or mod is None:
            continue
        for name in dir(mod):
            if not name.startswith(("_serialize", "_rust_", "rust_")):
                continue
            attr = getattr(mod, name)
            if not callable(attr) or getattr(attr, "_wire_prep_patched", False):
                continue
            # A bare `rust_` name is only a gap when it binds a kernel
            # function: mypy defines its own `rust_*` helpers that call
            # the wrapped `_rust_*` seams and are fine.
            if name.startswith("rust_") and getattr(attr, "__module__", "").startswith("mypy"):
                continue
            gaps.append(f"{modname}.{name}")
    return sorted(set(gaps))


def identity_failures() -> list[str]:
    fails = []
    consumed = (
        sum(prep_useful_ns.values())
        + sum(prep_deferred_ns.values())
        + sum(prep_raised_ns.values())
    )
    total = serialize_entry_ns
    accounted = consumed + unconsumed_ns + no_blob_ns + serialize_raised_ns
    if accounted != total:
        fails.append(
            f"ledger S identity: consumed {consumed} + unconsumed {unconsumed_ns} "
            f"+ no-blob {no_blob_ns} + raised {serialize_raised_ns} "
            f"!= entry total {total}"
        )
    for seam in set(prep_events):
        got = prep_useful_ns[seam] + prep_deferred_ns[seam] + prep_raised_ns[seam]
        # Zero-dt blobs (non-first elements of list results) make 0 ns a
        # legitimate per-seam total; only a lost nonzero entry is a hole.
        if prep_nonzero_events[seam] and got == 0:
            fails.append(
                f"seam {seam}: {prep_nonzero_events[seam]} nonzero prep "
                f"events, 0 ns attributed"
            )
    window_sum = sum(window_useful_ns.values()) + sum(window_deferred_ns.values())
    window_sum += sum(window_raised_ns.values())
    if window_sum == 0 and window_events:
        fails.append("ledger W: window events but zero window nanoseconds")
    return fails


# --- known-answer self-test --------------------------------------------------


def _fresh_any() -> Any:
    from mypy.types import AnyType, TypeOfAny

    return AnyType(TypeOfAny.explicit)


# Calibration rates are min-of-rounds per-event nanoseconds, so a single
# noisy round cannot inflate the quoted wrapper overhead.
_CAL_ROUNDS = 9
_CAL_N = 500


def calibrate() -> tuple[int, int]:
    """Per-event cost of the serialize wrapper vs the raw serializer.

    The ledger's per-event numbers include the wrapper bookkeeping
    (timing, frame lookup, site string, blob pinning), so an
    instrumented serialize event costs more than a raw one. This is the
    committed, reproducible source of the wrapper-vs-raw correction the
    #45 report relied on: run `--calibrate` on the machine a measurement
    came from and subtract the overhead when quoting absolute prep
    numbers. Rates are min-of-rounds over `_CAL_N` `_serialize_type`
    calls on rotating `AnyType` inputs, wire cache off. The raw rate is
    measured with every serializer temporarily unpatched, so it sees the
    true uninstrumented path, nested helpers included.
    """
    import mypy.subtypes
    import mypy.types

    raw = _orig_serializers.get("mypy.subtypes._serialize_type")
    if raw is None:
        raise RuntimeError(
            "calibration needs patch_serializers() to have captured "
            "mypy.subtypes._serialize_type first"
        )
    objs = [_fresh_any() for _ in range(8)]
    cache_state = mypy.types._type_wire_cache_enabled
    mypy.types._type_wire_cache_enabled = False
    try:

        def run_rounds(fn: Callable[..., Any], rounds: int) -> int:
            best: int | None = None
            for _ in range(rounds):
                t0 = _perf()
                for i in range(_CAL_N):
                    fn(objs[i % len(objs)])
                dt = _perf() - t0
                if best is None or dt < best:
                    best = dt
            assert best is not None
            return best // _CAL_N

        # One untimed pass per fn: lazy caches and first-touch costs of
        # a cold AnyType must not land on whichever fn is timed first.
        _restore_serializers()
        try:
            for i in range(_CAL_N):
                raw(objs[i % len(objs)])
            raw_ns = run_rounds(raw, _CAL_ROUNDS)
        finally:
            patch_serializers()
        wrapped = mypy.subtypes._serialize_type
        for i in range(_CAL_N):
            wrapped(objs[i % len(objs)])
        return raw_ns, run_rounds(wrapped, _CAL_ROUNDS)
    finally:
        mypy.types._type_wire_cache_enabled = cache_state


def selftest() -> list[str]:
    import type_kernel

    import mypy.constraints
    import mypy.subtypes
    import mypy.types

    fails: list[str] = []
    checks: list[tuple[str, bool, str]] = []

    def check(label: str, ok: bool, detail: str = "") -> None:
        checks.append((label, ok, detail))
        if not ok:
            fails.append(f"{label}: {detail}")

    n_unconsumed_before = unconsumed_events
    n_unc_events_before = serialize_entry_events
    n_pending_before = len(pending)

    # 1. An entry nobody consumes stays pending, then lands in the
    # unconsumed bucket exactly once at finalize (asserted in check 6).
    blob_orphan = mypy.subtypes._serialize_type(_fresh_any())
    check(
        "unconsumed entry counted",
        len(pending) == n_pending_before + 1 and serialize_entry_events == n_unc_events_before + 1,
        f"pending={len(pending)} (want +1), entries={serialize_entry_events} (want +1)",
    )
    check(
        "orphan blob still pending", id(blob_orphan) in pending, "orphan blob missing from pending"
    )

    # 2. Forced seam calls: N calls, one fresh prep blob each.
    n_ser = 40
    calls_before = seam_calls["rust_is_type_type"]
    events_before = prep_events["rust_is_type_type"]
    consumed_before = (
        prep_useful_ns["rust_is_type_type"]
        + prep_deferred_ns["rust_is_type_type"]
        + prep_raised_ns["rust_is_type_type"]
    )
    entry_total_before = serialize_entry_ns
    for _ in range(n_ser):
        blob = mypy.subtypes._serialize_type(_fresh_any())
        type_kernel.rust_is_type_type(blob)
    check(
        "N forced seam calls report exactly N prep events",
        seam_calls["rust_is_type_type"] - calls_before == n_ser
        and prep_events["rust_is_type_type"] - events_before == n_ser,
        f"calls={seam_calls['rust_is_type_type'] - calls_before} (want {n_ser}), "
        f"events={prep_events['rust_is_type_type'] - events_before} (want {n_ser})",
    )
    consumed_now = (
        prep_useful_ns["rust_is_type_type"]
        + prep_deferred_ns["rust_is_type_type"]
        + prep_raised_ns["rust_is_type_type"]
    )
    # The last iteration's blob is consumed; each earlier one too, so the
    # consumed delta must equal the entry-time delta for those calls.
    check(
        "per-seam prep totals sum to global prep total",
        consumed_now - consumed_before == serialize_entry_ns - entry_total_before,
        f"consumed delta {consumed_now - consumed_before} != "
        f"entry delta {serialize_entry_ns - entry_total_before}",
    )

    # 3. Wire-cache accumulation: two entries on one blob both land on
    # the next consumer (probe-local cache flip, restored below).
    mypy.types._type_wire_cache_enabled = True
    try:
        t = _fresh_any()
        total_before = serialize_entry_ns
        b1 = mypy.subtypes._serialize_type(t)
        dt1 = serialize_entry_ns - total_before
        mid = serialize_entry_ns
        b2 = mypy.subtypes._serialize_type(t)
        dt2 = serialize_entry_ns - mid
        check("wire-cache re-serve is the same blob", b1 is b2, "b1 is not b2")
        consumed_before2 = (
            prep_useful_ns["rust_is_type_type"]
            + prep_deferred_ns["rust_is_type_type"]
            + prep_raised_ns["rust_is_type_type"]
        )
        type_kernel.rust_is_type_type(b2)
        consumed_after2 = (
            prep_useful_ns["rust_is_type_type"]
            + prep_deferred_ns["rust_is_type_type"]
            + prep_raised_ns["rust_is_type_type"]
        )
        check(
            "both cache-hit preps attribute to the consumer",
            consumed_after2 - consumed_before2 == dt1 + dt2,
            f"delta {consumed_after2 - consumed_before2} != dt1+dt2 {dt1 + dt2}",
        )
    finally:
        mypy.types._type_wire_cache_enabled = False

    # 4. Adapter window: N adapter calls, one window each.
    n_win = 25
    win_events_before = window_events["rust_is_type_type"]
    win_ns_before = (
        window_useful_ns["rust_is_type_type"]
        + window_deferred_ns["rust_is_type_type"]
        + window_raised_ns["rust_is_type_type"]
    )
    for _ in range(n_win):
        mypy.constraints._try_native_is_type_type(_fresh_any())
    win_ns_after = (
        window_useful_ns["rust_is_type_type"]
        + window_deferred_ns["rust_is_type_type"]
        + window_raised_ns["rust_is_type_type"]
    )
    check(
        "N adapter calls report exactly N windows",
        window_events["rust_is_type_type"] - win_events_before == n_win,
        f"windows={window_events['rust_is_type_type'] - win_events_before} (want {n_win})",
    )
    check(
        "windows carry positive time",
        win_ns_after - win_ns_before > 0,
        f"window delta {win_ns_after - win_ns_before} ns",
    )

    # 5. Alias seam (from-imported `_rust_*`): consumption via the alias.
    n_alias = 15
    alias_events_before = prep_events["rust_has_type_vars"]
    alias_calls_before = seam_calls["rust_has_type_vars"]
    for _ in range(n_alias):
        blob = mypy.types._serialize_type_for_visitor(_fresh_any())
        mypy.types._rust_has_type_vars(blob)
    check(
        "alias seam reports its prep events",
        seam_calls["rust_has_type_vars"] - alias_calls_before == n_alias
        and prep_events["rust_has_type_vars"] - alias_events_before == n_alias,
        f"calls={seam_calls['rust_has_type_vars'] - alias_calls_before}, "
        f"events={prep_events['rust_has_type_vars'] - alias_events_before} "
        f"(want {n_alias}/{n_alias})",
    )

    # 6. Kwarg blob walk: nested containers in kwarg values are
    # consumed symmetrically with positional args.
    kw_probe = _wrap_seam("selftest_kw_probe", lambda *args, **kwargs: None, False)
    n_pending_kw = len(pending)
    b1 = mypy.subtypes._serialize_type(_fresh_any())
    b2 = mypy.subtypes._serialize_type(_fresh_any())
    kw_probe(x=[[b1], b2])
    check(
        "kwarg nested blob list is consumed",
        prep_events["selftest_kw_probe"] == 2 and len(pending) == n_pending_kw,
        f"prep events={prep_events['selftest_kw_probe']} (want 2), "
        f"pending delta {len(pending) - n_pending_kw} (want 0)",
    )

    # 7. A serialize that raises (PlaceholderType is unserializable by
    # design): the original exception propagates unchanged and the time
    # lands in the raised bucket, not in a seam's ledger.
    n_raised_before = serialize_raised_events
    raised_exc: BaseException | None = None
    try:
        mypy.subtypes._serialize_type(mypy.types.PlaceholderType("builtins.int", [], 1))
    except NotImplementedError as exc:
        raised_exc = exc
    check(
        "raised serialize propagates unchanged and is counted",
        isinstance(raised_exc, NotImplementedError)
        and serialize_raised_events == n_raised_before + 1,
        f"exc={raised_exc!r}, raised delta "
        f"{serialize_raised_events - n_raised_before} (want 1)",
    )

    # 8. Global conservation identity over the whole self-test. The
    # orphan blob from check 1 is still pending: finalize it first, or
    # the identity would count its entry time without a bucket.
    finalize_unconsumed()
    check(
        "orphan entry landed in the unconsumed bucket",
        unconsumed_events == n_unconsumed_before + 1 and not pending,
        f"unconsumed={unconsumed_events} (want +1), pending={len(pending)} (want 0)",
    )
    check("global identity holds", not identity_failures(), str(identity_failures()))
    gaps = coverage_census()
    check("coverage census clean", not gaps, f"unpatched: {gaps}")

    # 9. Idempotence: a second patch pass must not re-wrap. Pre-guard,
    # the bare `rust_*`/type_kernel pass re-wrapped every seam, so one
    # logical seam call counted twice in every ledger.
    n_reg_before = len(registered_seams)
    n_repatched = patch_seams()
    calls_repatch = seam_calls["rust_is_type_type"]
    type_kernel.rust_is_type_type(mypy.subtypes._serialize_type(_fresh_any()))
    check(
        "second patch_seams() wraps nothing and counts once",
        n_repatched == 0
        and len(registered_seams) == n_reg_before
        and seam_calls["rust_is_type_type"] - calls_repatch == 1,
        f"repatched={n_repatched} (want 0), call delta "
        f"{seam_calls['rust_is_type_type'] - calls_repatch} (want 1)",
    )
    adapter_fn = mypy.constraints._try_native_is_type_type
    patch_adapters()
    check(
        "second patch_adapters() rewraps nothing",
        mypy.constraints._try_native_is_type_type is adapter_fn,
        "adapter function object changed across a re-patch",
    )

    # 10. Calibration: the instrumented wrapper must cost more per
    # event than the raw serializer; both rates print as the detail.
    raw_ns, wrapped_ns = calibrate()
    check(
        "calibration: instrumented wrapper costs more than raw",
        wrapped_ns > raw_ns,
        f"raw {raw_ns} ns/event vs wrapper {wrapped_ns} ns/event",
    )
    finalize_unconsumed()
    check("identity holds after calibration", not identity_failures(), str(identity_failures()))

    print("=== wire-prep probe known-answer validation ===", file=sys.stderr)
    for label, ok, detail in checks:
        mark = "PASS" if ok else "FAIL"
        line = f"[{mark}] {label}"
        if detail and not ok:
            line += f": {detail}"
        print(line, file=sys.stderr)
    return fails


# --- report ------------------------------------------------------------------


def _fmt_s(ns: float) -> str:
    return f"{ns / 1e9:.3f}s"


def _defer_share(seam: str) -> float:
    calls = seam_calls[seam]
    return (100.0 * seam_defers[seam] / calls) if calls else 0.0


def report(run_status: str) -> None:
    out = sys.stderr
    import mypy as _mypy

    prep_s_total = sum(prep_useful_ns.values()) + sum(prep_deferred_ns.values())
    prep_s_total += sum(prep_raised_ns.values())
    prep_w_total = (
        sum(window_useful_ns.values())
        + sum(window_deferred_ns.values())
        + sum(window_raised_ns.values())
    )
    prep_deferred_total = sum(prep_deferred_ns.values()) + sum(window_deferred_ns.values())
    print("\n=== #1878 wire-prep measurement (cold self-check) ===", file=out)
    print(f"[prep] {len(registered_seams)} seams registered", file=out)
    print(f"source tree: {_mypy.__file__}", file=out)
    print(f"audited run: {run_status}", file=out)
    for line in audit.extension_identity(_mypy):
        print(line, file=out)
    splice = getattr(_mypy.types, "write_raw_bytes", None)
    print(
        f"librt splice: {'ACTIVE' if splice is not None else 'INACTIVE'} "
        f"(write_raw_bytes {'present' if splice is not None else 'absent'}; "
        f"_write_type_cached {'splices' if splice is not None else 'degrades to t.write'})",
        file=out,
    )
    print(
        f"serialize entries: {serialize_entry_events} events, "
        f"{_fmt_s(serialize_entry_ns)} "
        f"(raised-before-blob {serialize_raised_events}, {_fmt_s(serialize_raised_ns)})",
        file=out,
    )
    print(
        f"  attributed to seams: {_fmt_s(prep_s_total)} "
        f"(useful {_fmt_s(sum(prep_useful_ns.values()))} / "
        f"deferred {_fmt_s(sum(prep_deferred_ns.values()))} / "
        f"raised {_fmt_s(sum(prep_raised_ns.values()))})",
        file=out,
    )
    print(
        f"  unconsumed: {unconsumed_events} pending blobs (per blob, not per "
        f"entry; 0-dt non-first blobs of multi-blob results included, so "
        f"this can exceed serialize entries), {_fmt_s(unconsumed_ns)} | "
        f"no-blob results: {no_blob_events}, {_fmt_s(no_blob_ns)}",
        file=out,
    )
    print(
        f"adapter windows: {sum(window_events.values())} events, "
        f"{_fmt_s(prep_w_total)} (unattributed discards "
        f"{_fmt_s(window_unattributed_ns)})",
        file=out,
    )
    print(
        f"  adapter residual: {_fmt_s(adapter_residual_called_ns + adapter_residual_uncalled_ns)} "
        f"= window time no ledger sees (raw t.write buffer prep, gate "
        f"checks, argument glue);",
        file=out,
    )
    print(
        f"  seam-called windows {_fmt_s(adapter_residual_called_ns)} / "
        f"never-called windows {_fmt_s(adapter_residual_uncalled_ns)} "
        f"(the never-called share is prep for seam calls the adapter never made)",
        file=out,
    )
    seam_attributed_ns = prep_s_total + prep_w_total + serialize_raised_ns
    print(
        f"TOTAL seam-attributed wire prep: {_fmt_s(seam_attributed_ns)} "
        f"(deferred-call share {_fmt_s(prep_deferred_total)}; "
        f"raised-serialize share {_fmt_s(serialize_raised_ns)})",
        file=out,
    )
    print(
        f"  + unconsumed prep (never reached a seam): {unconsumed_events} "
        f"pending blobs, {_fmt_s(unconsumed_ns)}; the two sum to "
        f"{_fmt_s(seam_attributed_ns + unconsumed_ns)}",
        file=out,
    )
    identity = identity_failures()
    print(f"identity check: {'PASS' if not identity else 'FAIL'}", file=out)
    for fail in identity:
        print(f"  {fail}", file=out)

    print("\n--- R. ranked pre-gate candidates (prep on deferred calls) ---", file=out)
    instr_rate = os.environ.get("MYPY_PREP_INSTR_PER_SEC")
    rows = []
    for seam in seam_calls:
        deferred_ns = prep_deferred_ns[seam] + window_deferred_ns[seam]
        if deferred_ns == 0:
            continue
        rows.append((deferred_ns, seam))
    rows.sort(reverse=True)
    print(
        "  rank  seam                                        calls   defer%  "
        "prep_def_s   prep_total_s  est_instr_saved",
        file=out,
    )
    for rank, (deferred_ns, seam) in enumerate(rows[:35], 1):
        total_ns = prep_useful_ns[seam] + prep_deferred_ns[seam] + prep_raised_ns[seam]
        total_ns += window_useful_ns[seam] + window_deferred_ns[seam]
        total_ns += window_raised_ns[seam]
        est = "n/a"
        if instr_rate:
            est = f"{int(deferred_ns * float(instr_rate) / 1e9):.3e}"
        print(
            f"  {rank:4d}  {seam:<44} {seam_calls[seam]:>8} "
            f"{_defer_share(seam):7.1f}  {deferred_ns / 1e9:10.3f}  "
            f"{total_ns / 1e9:12.3f}  {est}",
            file=out,
        )

    print("\n--- P. per-seam prep (all seams with prep or calls, by total prep) ---", file=out)
    all_seams = set(seam_calls) | set(prep_events) | set(window_events)
    prow = []
    for seam in all_seams:
        total_ns = prep_useful_ns[seam] + prep_deferred_ns[seam] + prep_raised_ns[seam]
        wns = window_useful_ns[seam] + window_deferred_ns[seam] + window_raised_ns[seam]
        prow.append((total_ns + wns, seam, total_ns, wns))
    prow.sort(reverse=True)
    print(
        "  seam                                        calls  defers  "
        "prep_S_s(useful/deferred/raised)  prep_W_s  seam_call_in_window_s",
        file=out,
    )
    print(
        "  (seam_call_in_window_s accumulates only calls timed inside an "
        "adapter window; it is 0 for seams called outside adapters)",
        file=out,
    )
    for total_ns, seam, s_ns, w_ns in prow[:45]:
        parts = (
            f"{prep_useful_ns[seam] / 1e9:.3f}/"
            f"{prep_deferred_ns[seam] / 1e9:.3f}/"
            f"{prep_raised_ns[seam] / 1e9:.3f}"
        )
        alias = " (alias)" if seam in alias_seams else ""
        print(
            f"  {seam + alias:<48} {seam_calls[seam]:>8} {seam_defers[seam]:>7} "
            f"{parts:>22}  {w_ns / 1e9:9.3f}  {seam_call_ns[seam] / 1e9:9.3f}",
            file=out,
        )

    print("\n--- S. top serialize call sites ---", file=out)
    for site, ns in site_ns.most_common(25):
        print(f"  {ns / 1e9:9.3f}s  {site_events[site]:>9}  {site}", file=out)

    print("\n--- U. top unconsumed prep sites (never reached a seam) ---", file=out)
    for site, ns in unconsumed_site_ns.most_common(15):
        print(f"  {ns / 1e9:9.3f}s  {site}", file=out)

    print("\n--- RAISE. top serialize sites that raised (native attempt abandoned) ---", file=out)
    for site, ns in raised_site_ns.most_common(10):
        print(f"  {ns / 1e9:9.3f}s  {site}", file=out)

    zero = [s for s in seam_calls if not prep_events[s] and not window_events[s]]
    print(f"\n--- Z. called seams with zero prep: {len(zero)} of {len(seam_calls)} ---", file=out)

    gaps = coverage_census()
    print(f"\n--- coverage census: {len(gaps)} unpatched helper(s) ---", file=out)
    for gap in gaps:
        print(f"  GAP {gap}", file=out)


# --- main ---------------------------------------------------------------------

PREP_REFUSAL = (
    "measure_wire_prep: HOLLOW PREP READING.\n"
    "  {detail}\n"
    "  The report above cannot be read as evidence. Remedy: run the pinned\n"
    "  single-process invocation with the extension scratch dirs on PYTHONPATH."
)


def finalize_unconsumed() -> None:
    """Move still-pending blobs to the unconsumed bucket.

    Counts per blob (multi-blob results contribute more than one), so
    the event count is not comparable to serialize entries.
    """
    global unconsumed_ns, unconsumed_events
    for key, entry in pending.items():
        unconsumed_ns += entry[0]
        unconsumed_events += 1
        unconsumed_site_ns[entry[1]] += entry[0]
    pending.clear()


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        _import_all()
        try:
            patch_seams()
        except audit.KernelUnavailableError:
            return 1
        n_ser = patch_serializers()
        patch_adapters()
        print(f"[prep] self-test harness: {n_ser} serializers patched", file=sys.stderr)
        fails = selftest()
        if fails:
            print(f"measure_wire_prep: {len(fails)} self-test failure(s)", file=sys.stderr)
            return 1
        print("self-test: all known-answer checks passed", file=sys.stderr)
        return 0

    if "--calibrate" in argv:
        _import_all()
        patch_serializers()
        raw_ns, wrapped_ns = calibrate()
        print("=== wire-prep probe calibration: serialize wrapper overhead ===", file=sys.stderr)
        print(f"raw _serialize_type:            {raw_ns} ns/event", file=sys.stderr)
        print(f"instrumented (ledger wrapper): {wrapped_ns} ns/event", file=sys.stderr)
        print(f"wrapper overhead:              {wrapped_ns - raw_ns} ns/event", file=sys.stderr)
        print(
            "ledger S event cost includes this overhead; subtract it when quoting "
            "absolute prep numbers (min-of-rounds rate, wire cache off)",
            file=sys.stderr,
        )
        return 0

    # Measurement run: single-process cold self-check via the audited argv.
    args = audit.audit_argv(os.environ.get("MYPY_AUDIT_ARGS"))
    refusal = audit.fanout_reason(args, os.environ)
    if refusal is not None:
        print(refusal, file=sys.stderr)
        return 1
    _import_all()
    try:
        patch_seams()
    except audit.KernelUnavailableError:
        return 1
    import mypy.main

    n_ser = patch_serializers()
    n_ad = patch_adapters()
    print(
        f"[prep] patched {len(registered_seams)} seams "
        f"({len(alias_seams)} via module aliases), {n_ser} serializers, "
        f"{n_ad} adapters",
        file=sys.stderr,
    )
    sys.argv = args
    run_failure: str | None = None
    run_status = "not started"
    t0 = _perf()
    try:
        mypy.main.main(clean_exit=True)
        run_status = "completed, mypy.main returned"
    except SystemExit as exc:
        run_failure = audit.classify_run_exit(exc.code)
        if run_failure is None:
            run_status = f"completed, mypy exit {exc.code!r}"
        else:
            run_status = f"FAILED, {run_failure}"
    except KeyboardInterrupt:
        print("measure_wire_prep: aborted by operator; no report", file=sys.stderr)
        raise
    except BaseException as exc:
        run_failure = f"{type(exc).__name__}: {exc}"
        run_status = f"FAILED, raised {run_failure}"
        import traceback

        traceback.print_exc()
    finally:
        wall_ns = _perf() - t0
    finalize_unconsumed()
    report(run_status)
    print(f"[prep] instrumented run wall {wall_ns / 1e9:.1f}s", file=sys.stderr)

    refusals = audit.evidence_refusals(
        sum(seam_calls.values()), len(registered_seams), run_failure
    )
    if serialize_entry_events == 0 and sum(window_events.values()) == 0:
        refusals.append(
            PREP_REFUSAL.format(
                detail=(
                    f"seams were called {sum(seam_calls.values())} times but the "
                    f"probe recorded zero serialize entries and zero adapter "
                    f"windows: every prep path is dark or uninstrumented."
                )
            )
        )
    gaps = coverage_census()
    if gaps:
        refusals.append(
            PREP_REFUSAL.format(detail=f"unpatched helpers would undercount prep: {gaps[:10]}")
        )
    identity = identity_failures()
    if identity:
        refusals.append(PREP_REFUSAL.format(detail="; ".join(identity[:3])))
    for reason in refusals:
        print(reason, file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
