"""Measure the native work share by call-count, independent of wall-clock.

The time-based metric ((python - native) / python) has inverted since M17:
accumulated per-call wire/deferral overhead makes the native path slower
than pure Python even though more work runs in Rust. This script measures
what the user actually asked for: the fraction of kernel seam *calls*
that Rust handles successfully (result not None), i.e. how much of the
work really executes in Rust.

It installs a counting proxy over every `rust_*` attribute of the
`type_kernel` extension module *before* any mypy module imports it, then
runs the cold self-check. Both import styles (`import type_kernel as
_tk` and `from type_kernel import rust_x`) bind attributes at import
time, so the proxy is seen by both.

`rust_is_subtype_batch` returns a per-pair decision list (1/0 answered,
-1 deferred); its slots are unwrapped so each decision counts as native
or fallback, matching how the direct seams are counted per call. The
single-pair `rust_is_subtype_coded` (#33) returns one code per call
(1/0/3 answered, -1 deferred) and is classified the same way. This
keeps the metric per-unit-of-work rather than per-seam-call.

Usage:
    uv run python scripts/measure_native_share.py [--only-native]

Reports:
    total calls, native successes, python fallbacks, and per-name share.
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Callable
from types import ModuleType
from typing import Any

# rust_* classifier seams whose None is the decided negative, not a deferral
# (e.g. "not a TypeVar declaration"); counting them as fallbacks understates
# the ported share. Re-measure before extending: only settled negatives count.
CLASSIFIER_NEGATIVE_SEAMS: tuple[str, ...] = (
    "rust_get_typevarlike_declaration",
    "rust_find_dataclass_transform_spec",
    "rust_find_duplicate",
    # visit_call_expr callee classifier: None = "not a protocol test"
    # (arity != 2, non-RefExpr, or wrong fullname); all arms decided.
    "rust_classify_protocol_test_callee",
)

# Batch seam returning a per-pair list: 1/0 = native, -1 = deferral.
# Count per-decision, not per-call, so deferred slots stay visible.
_BATCH_SLOT_SEAMS: frozenset[str] = frozenset({"rust_is_subtype_batch"})

# Coded single-pair seam (#33): 1/0/3 = native decision (3 = consult
# cut, still decided), -1 = deferral. Never None, so without this rule
# every deferral would count as native.
_CODED_DECISION_SEAMS: frozenset[str] = frozenset({"rust_is_subtype_coded"})

# Unit-handled seams: Rust returns None (PyOk unit) as the HANDLED result,
# not a deferral (semanal_classprop full ports; shim returns on success).
# Without this, every successful call would count as a fallback.
UNIT_HANDLED_SEAMS: frozenset[str] = frozenset(
    {
        "rust_calculate_class_abstract_status",
        "rust_check_protocol_status",
        "rust_calculate_class_vars",
        "rust_add_type_promotion",
    }
)


class CountingProxy:
    """Wrap one rust_* function, counting calls and None (deferral) results."""

    __slots__ = ("name", "wrapped", "calls", "native", "fallback")

    def __init__(self, name: str, wrapped: Callable[..., Any]) -> None:
        self.name = name
        self.wrapped = wrapped
        self.calls = 0
        self.native = 0
        self.fallback = 0

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        self.calls += 1
        result = self.wrapped(*args, **kwargs)
        if self.name in _BATCH_SLOT_SEAMS:
            # Per-decision: each answered slot is native work, each -1 is
            # a deferral. The seam itself always returns a list (never
            # None), so without unwrapping every batch would count native.
            if isinstance(result, list):
                for slot in result:
                    if slot == -1:
                        self.fallback += 1
                    else:
                        self.native += 1
            else:
                self.native += 1
        elif self.name in _CODED_DECISION_SEAMS:
            # Per-decision single-pair code: 1/0/3 native, -1 deferral.
            if result == -1:
                self.fallback += 1
            else:
                self.native += 1
        elif result is None and self.name not in CLASSIFIER_NEGATIVE_SEAMS:
            if self.name in UNIT_HANDLED_SEAMS:
                # Unit seam: None is the handled result, not a deferral.
                self.native += 1
            else:
                self.fallback += 1
        else:
            self.native += 1
        return result


def install_proxy(kernel: ModuleType) -> dict[str, CountingProxy]:
    """Replace every rust_* attribute of the kernel with a CountingProxy.

    mutating setattr on the module object is visible to later attribute
    reads (`mypy.constraints._type_kernel.rust_x` resolves through it).
    """
    proxies: dict[str, CountingProxy] = {}
    for name in dir(kernel):
        if not name.startswith("rust_"):
            continue
        attr = getattr(kernel, name)
        if not callable(attr):
            continue
        proxy = CountingProxy(name, attr)
        setattr(kernel, name, proxy)
        proxies[name] = proxy
    return proxies


def run(cwd: str) -> dict[str, CountingProxy]:
    # Import the kernel on a probe path first so the proxy installation
    # happens before any mypy module imports it. Both import styles
    # bind module attributes, so the proxy is seen by all.
    import type_kernel

    proxies = install_proxy(type_kernel)
    import mypy.main

    sys.argv = [
        "mypy",
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
    # clean_exit disables fast_exit so main() raises SystemExit (with the
    # self-check's expected 230-error code) instead of hard-killing the
    # process before we can report the counters.
    try:
        mypy.main.main(clean_exit=True)
    except SystemExit:
        pass
    return proxies


def per_seam_report(proxies: dict[str, CountingProxy]) -> list[str]:
    """Per-seam display lines (#36: deferrals must survive the display).

    The deferral list ranks by deferral count, not call count: a seam
    with few calls but real deferrals is exactly the row the reader
    needs, and call-count ranking hid the coded subtype seam's 64
    deferrals in the #35 review. Every share prints with two decimals,
    because a 99.72% native seam rounded to "100%" hid the same number.
    """
    lines: list[str] = []
    lines.append("")
    lines.append("top deferrals by deferral count:")
    for name, p in sorted(proxies.items(), key=lambda kv: kv[1].fallback, reverse=True)[:15]:
        if p.fallback:
            lines.append(
                f"  {name}: {p.calls} calls, {p.fallback} fallbacks "
                f"({100.0 * p.fallback / p.calls:.2f}% defer)"
            )
    lines.append("")
    lines.append("all seams with calls:")
    for name, p in sorted(proxies.items(), key=lambda kv: kv[1].calls, reverse=True):
        if p.calls:
            lines.append(f"  {name}: {p.calls} calls ({100.0 * p.native / p.calls:.2f}% native)")
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    proxies = run(os.getcwd())
    out = sys.stderr  # self-check floods stdout; report on stderr
    total = sum(p.calls for p in proxies.values())
    native = sum(p.native for p in proxies.values())
    fallback = total - native
    print("\n=== native call share (self-check) ===", file=out)
    print(f"total seam calls: {total}", file=out)
    if total:
        print(f"native:          {native} ({100.0 * native / total:.1f}%)", file=out)
        print(f"python fallback: {fallback} ({100.0 * fallback / total:.1f}%)", file=out)
        for line in per_seam_report(proxies):
            print(line, file=out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
