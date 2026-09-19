#!/usr/bin/env python3
"""#66 Phase 1: resolver-snapshot churn profile on the cold self-check.

Instruments consecutive `_build_native_resolvers` collects in a real cold
single-process self-check (`mypy_self_check.ini -n0 --no-incremental
-p mypy -p mypyc`). Delivers the falsifier evidence for issue #66:
per-collect entry counts, the actual re-push feed, per-phase timers,
`write_str` attribution inside vs outside the resolver build, and (with
`--sigs`) a byte signature of every re-collected snapshot entry compared
against its previous appearance — the unchanged fraction the pre-set
gate turns on.

The signature is a Python mirror of `snapshot_type_info`
(crates/type_kernel/src/typeinfo.rs): same fields, same defaults on
failure, type fields serialized through the same `Type.write` wire path
the Rust updater uses. Identical signatures mean the Rust re-snapshot
stored byte-identical content. A changed entry additionally reports
which fields moved and why its module was walked (`in_scc`, `new`,
`builtins`), mirroring `_collect_incremental`'s walk conditions.

Usage: .venv/bin/python misc/resolver_snap_phase_profile.py [--sigs] [-- extra args]
Corpus class: /private/tmp/mypy-rs-sem.sh run 2 <this script>.
"""

from __future__ import annotations

import os
import sys
import time
from dataclasses import dataclass

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


@dataclass
class ChangedEntry:
    """A re-collected entry whose signature differs from its previous
    appearance, with the fields that moved and why it was walked."""

    collect_idx: int
    fullname: str
    module: str
    reason: str
    repushed: bool
    prev_len: int
    sig_len: int
    fields: list[tuple[str, int, int]]  # field name, prev bytes, cur bytes


class State:
    """All counters and timing accumulators. Single process, single thread."""

    def __init__(self, sigs: bool) -> None:
        self.sigs = sigs
        self.in_build = False
        self.in_first_build = False
        self.in_probe_sig = False
        # Collects.
        self.collects = 0
        self.first_collects = 0
        self.empty_scc_collects = 0
        # Population counts (mirror the #61 audit's semantics).
        self.collected_infos = 0
        self.collected_aliases = 0
        self.new_infos = 0
        self.repush = 0
        self.recollected = 0  # already-snapshotted entries walked again
        self.recollected_builtins = 0  # the audit's re_pushed_builtins
        # Timers (seconds).
        self.build_s = 0.0
        self.first_build_s = 0.0
        self.collect_s = 0.0
        self.split_s = 0.0
        self.sig_s = 0.0  # probe-only signature overhead (not run cost)
        self.ctor_blob_s = 0.0  # post-first collects only
        self.ctor_blob_first_s = 0.0
        self.first_collect_s = 0.0
        self.sig_in_build_s = 0.0  # probe sig work inside the build window
        # write_str attribution (probe's own signature writes excluded).
        self.build_write_str = 0
        self.total_write_str = 0
        # Signature comparisons (--sigs mode).
        self.comparisons = 0
        self.unchanged = 0
        self.recollected_comparisons = 0
        self.recollected_unchanged = 0
        self.repush_comparisons = 0
        self.repush_unchanged = 0
        self.repush_bytes = 0
        self.repush_unchanged_bytes = 0
        self.no_baseline = 0
        self.mirror_failures = 0
        self.changed_entries: list[ChangedEntry] = []
        self.sigs_stored: dict[str, dict[str, bytes]] = {}
        self.last_collect: tuple[list, list] | None = None
        self.saw_first = False
        # Walk-reason attribution for changed entries.
        self.manager = None
        self.current_scc: list[str] | None = None
        self.walked_before: set[str] | None = None
        self.reason_cache: tuple[int, dict[str, str]] | None = None


def _write_type(t) -> bytes:
    from librt.internal import WriteBuffer

    buf = WriteBuffer()
    t.write(buf)
    return buf.getvalue()


def snap_signature(info) -> dict[str, bytes]:
    """Mirror `snapshot_type_info` field-for-field, deterministically.

    Returns one byte string per Rust-read field. The values concatenated
    in insertion order are byte-identical to the previous single-buffer
    signature, so dict equality and blob equality agree. Byte-identity
    between two appearances means the Rust updater stored identical
    content both times. Failure defaults mirror the Rust reads
    (`unwrap_or_default`, skip-on-failure); never raises.
    """
    fields: dict[str, bytes] = {}
    cur = bytearray()

    def flush(name: str) -> None:
        fields[name] = bytes(cur)
        cur.clear()

    def s(x: str) -> None:
        e = x.encode()
        cur.extend(len(e).to_bytes(8, "little") + e)

    def b(x: bool) -> None:
        cur.extend(b"\x01" if x else b"\x00")

    def i(x: int) -> None:
        cur.extend(int(x).to_bytes(8, "little", signed=True))

    def opt_i(x) -> None:
        if x is None:
            cur.extend(b"\xff" * 8)
        else:
            i(x)

    def strs(xs) -> None:
        i(len(xs))
        for x in xs:
            s(x)

    def blob_list(xs) -> None:
        """Serialize a list of type blobs, skipping items that fail."""
        blobs = []
        for x in xs:
            try:
                blobs.append(_write_type(x))
            except Exception:
                continue
        i(len(blobs))
        for blob in blobs:
            i(len(blob))
            cur.extend(blob)

    fullname = info.fullname
    s(fullname)
    flush("fullname")
    try:
        s(info.name)
    except Exception:
        s(fullname.rsplit(".", 1)[-1] if "." in fullname else fullname)
    flush("name")
    for attr in (
        "is_protocol",
        "is_enum",
        "fallback_to_any",
        "meta_fallback_to_any",
        "is_named_tuple",
        "is_newtype",
        "has_type_var_tuple_type",
        "has_param_spec_type",
        "is_abstract",
    ):
        try:
            b(bool(getattr(info, attr)))
        except Exception:
            b(False)
    flush("flags")
    try:
        strs(info.enum_members)
    except Exception:
        strs([])
    flush("enum_members")
    try:
        strs(list(info.type_vars))
    except Exception:
        strs([])
    flush("type_vars")
    try:
        mro = [x.fullname for x in info.mro]
    except Exception:
        mro = []
    strs(mro)
    flush("mro")
    try:
        pm = [x for x in info.protocol_members if isinstance(x, str)]
    except Exception:
        pm = []
    strs(pm)
    flush("protocol_members")
    try:
        blob_list(info._promote)
    except Exception:
        cur.extend(b"\x00" * 8)
    flush("promote")
    try:
        alt = info.alt_promote
        if alt is None:
            s("\x00none")
        else:
            s(alt.type.fullname)
    except Exception:
        s("\x00unreadable")
    flush("alt_promote")
    try:
        mc = info.metaclass_type
        s("" if mc is None else mc.type.fullname)
    except Exception:
        s("\x00unreadable")
    flush("metaclass")
    try:
        blob_list(info.bases)
    except Exception:
        cur.extend(b"\x00" * 8)
    flush("bases")
    try:
        tt = info.tuple_type
        cur.extend(b"\x00" if tt is None else b"\x01")
        if tt is not None:
            blob = _write_type(tt)
            i(len(blob))
            cur.extend(blob)
    except Exception:
        cur.extend(b"\x00")
    flush("tuple_type")
    try:
        opt_i(info.type_var_tuple_prefix)
    except Exception:
        opt_i(None)
    flush("tvt_prefix")
    try:
        opt_i(info.type_var_tuple_suffix)
    except Exception:
        opt_i(None)
    flush("tvt_suffix")
    # defn.type_vars: (name, variance, kind, upper-bound blob). Mirror the
    # class-name dispatch, the skip-on-unreadable-name rule, and the
    # kind-0-only upper bound.
    try:
        tvars = list(info.defn.type_vars)
    except Exception:
        tvars = []
    # Separate walk mirroring read_type_var_tuple_fallback: the first
    # TypeVarTupleType's tuple_fallback, independent of name readability.
    tvt_fallback: bytes | None = None
    for tv in tvars:
        if type(tv).__name__ == "TypeVarTupleType":
            try:
                tvt_fallback = _write_type(tv.tuple_fallback)
            except Exception:
                tvt_fallback = None
            break
    cur.extend(b"\xff" if tvt_fallback is None else b"\x01")
    if tvt_fallback is not None:
        i(len(tvt_fallback))
        cur.extend(tvt_fallback)
    flush("tvt_fallback")
    named = []
    for tv in tvars:
        try:
            name = tv.name
        except Exception:
            continue
        named.append((tv, name))
    i(len(named))
    for tv, name in named:
        cls = type(tv).__name__
        if cls == "TypeVarType":
            kind, variance = 0, int(getattr(tv, "variance", 0))
            try:
                ub = _write_type(tv.upper_bound)
            except Exception:
                ub = b""
        else:
            kind = 1 if cls == "ParamSpecType" else 2 if cls == "TypeVarTupleType" else 0
            variance = 0
            ub = b""
        s(name)
        i(variance)
        i(kind)
        i(len(ub))
        cur.extend(ub)
    flush("tvars")
    for tv in tvars:
        # read_type_var_raw_ids walks every defn.type_vars item (no name
        # filter), -1 on unreadable id.
        try:
            i(tv.id.raw_id)
        except Exception:
            i(-1)
    flush("raw_ids")
    # member_info + member_definers over info.names, sorted for determinism.
    try:
        items = sorted((k, v) for k, v in info.names.items())
    except Exception:
        items = []
    i(len(items))
    for name, sym in items:
        s(name)
        try:
            b(bool(sym.implicit))
        except Exception:
            b(False)
        try:
            node = sym.node
        except Exception:
            node = None
        try:
            b(bool(node.has_explicit_value))
        except Exception:
            b(False)
        cls = type(node).__name__ if node is not None else ""
        if cls in ("FuncDef", "OverloadedFuncDef"):
            kind = 0
        elif cls == "Decorator":
            kind = 1
        elif cls == "Var":
            kind = 2
        else:
            kind = -1
        i(kind)
        if kind < 0:
            s("")
        else:
            try:
                s(node.info.fullname)
            except Exception:
                s("")
    flush("members")
    return fields


def sig_len(sig: dict[str, bytes]) -> int:
    """Length of the single-buffer signature (concatenated fields)."""
    return sum(len(v) for v in sig.values())


def entry_reason(state: State, info) -> str:
    """Which walked modules yielded `info` at the current collect, and why.

    A TypeInfo is collected via every walked module whose symbol table
    references it (imported names share the node), not only via its
    defining module, so the reason is attributed to the walked modules
    that actually yield the entry, using `_collect_incremental`'s walk
    conditions (in_scc, new, builtins).
    """
    if state.reason_cache is None or state.reason_cache[0] != state.collects:
        mgr = state.manager
        if mgr is None:
            return "no-manager"
        scc_set = set(state.current_scc or ())
        reasons: dict[str, str] = {}
        for mod_id, mod in mgr.modules.items():
            if mod is None:
                continue
            if mod_id in scc_set:
                reasons[mod_id] = "in_scc"
            elif mod_id not in state.walked_before:
                reasons[mod_id] = "new"
            elif mod_id == "builtins":
                reasons[mod_id] = "builtins"
        state.reason_cache = (state.collects, reasons)
    mgr = state.manager
    from mypy.nodes import TypeInfo

    found = []
    for mod_id, reason in state.reason_cache[1].items():
        module = mgr.modules.get(mod_id)
        if module is None:
            continue
        stack = [sym.node for sym in module.names.values()]
        while stack:
            node = stack.pop()
            if node is info:
                found.append(f"{reason}:{mod_id}")
                break
            if isinstance(node, TypeInfo):
                stack.extend(sym.node for sym in node.names.values())
    return ",".join(found) if found else "not-walked"


def compare(state: State, infos: list, snapshotted: set, repush_set: set) -> None:
    """Store signatures for every collected entry; compare re-collected
    entries (already snapshotted) against their previous appearance."""
    t0 = time.perf_counter()
    state.in_probe_sig = True
    try:
        for info in infos:
            fullname = info.fullname
            already = fullname in snapshotted
            if already:
                state.recollected += 1
                if fullname.startswith("builtins."):
                    state.recollected_builtins += 1
            try:
                sig = snap_signature(info)
            except Exception:
                # The Rust reader never raises; a mirror failure is a probe
                # defect. Count it and refuse the evidence at the end.
                state.mirror_failures += 1
                continue
            prev = state.sigs_stored.get(fullname)
            if prev is None:
                state.no_baseline += 1
            elif already:
                state.comparisons += 1
                state.recollected_comparisons += 1
                same = prev == sig
                cur_len = sig_len(sig)
                if same:
                    state.unchanged += 1
                    state.recollected_unchanged += 1
                elif len(state.changed_entries) < 64:
                    try:
                        module = info.module_name
                    except Exception:
                        module = ""
                    state.changed_entries.append(
                        ChangedEntry(
                            collect_idx=state.collects,
                            fullname=fullname,
                            module=module,
                            reason=entry_reason(state, info),
                            repushed=fullname in repush_set,
                            prev_len=sig_len(prev),
                            sig_len=cur_len,
                            fields=sorted(
                                (name, len(prev[name]), len(value))
                                for name, value in sig.items()
                                if prev[name] != value
                            ),
                        )
                    )
                if fullname in repush_set:
                    state.repush_comparisons += 1
                    state.repush_bytes += cur_len
                    if same:
                        state.repush_unchanged += 1
                        state.repush_unchanged_bytes += cur_len
            state.sigs_stored[fullname] = sig
    finally:
        state.in_probe_sig = False
        elapsed = time.perf_counter() - t0
        state.sig_s += elapsed
        if state.in_build:
            # Only the compare runs inside the timed build window; the
            # first-build baseline compare (outside it) is excluded here.
            state.sig_in_build_s += elapsed


def install(state: State) -> None:
    import mypy.build as mb
    import mypy.types as mt

    orig_build = mb.BuildManager._build_native_resolvers
    orig_collect = mb.BuildManager._collect_incremental
    orig_split = mb._split_native_pending
    orig_ctor_blob = mb._native_ctor_blob
    orig_write_str = mt.write_str

    def counting_write_str(data, value):
        if state.in_probe_sig:
            # Probe signature writes are excluded from both counters; only
            # the checker's own wire traffic is attributed.
            return orig_write_str(data, value)
        if state.in_build:
            state.build_write_str += 1
        state.total_write_str += 1
        return orig_write_str(data, value)

    mt.write_str = counting_write_str

    def collect(self, scc):
        state.manager = self
        state.current_scc = scc
        state.walked_before = set(self._native_walked_modules)
        t0 = time.perf_counter()
        try:
            result = orig_collect(self, scc)
        finally:
            elapsed = time.perf_counter() - t0
            state.collect_s += elapsed
            if state.collects == 1:
                state.first_collect_s += elapsed
        state.last_collect = result
        return result

    def split(type_infos, snapshotted, prev_sigs):
        t0 = time.perf_counter()
        try:
            new_infos, repush, cur_sigs = orig_split(type_infos, snapshotted, prev_sigs)
        finally:
            state.split_s += time.perf_counter() - t0
        state.new_infos += len(new_infos)
        state.repush += len(repush)
        if state.sigs:
            compare(state, type_infos, snapshotted, {i.fullname for i in repush})
        else:
            state.recollected += sum(1 for i in type_infos if i.fullname in snapshotted)
            state.recollected_builtins += sum(
                1
                for i in type_infos
                if i.fullname in snapshotted and i.fullname.startswith("builtins.")
            )
        return new_infos, repush, cur_sigs

    def build(self, scc):
        state.collects += 1
        if not scc:
            state.empty_scc_collects += 1
        first = self._native_resolver is None
        t0 = time.perf_counter()
        state.in_build = True
        state.in_first_build = first
        try:
            result = orig_build(self, scc)
        finally:
            state.in_build = False
            state.in_first_build = False
            elapsed = time.perf_counter() - t0
            state.build_s += elapsed
            if first:
                state.first_collects += 1
                state.first_build_s += elapsed
        infos = state.last_collect[0] if state.last_collect else []
        state.collected_infos += len(infos)
        state.collected_aliases += len(state.last_collect[1]) if state.last_collect else 0
        if first:
            if state.saw_first:
                raise RuntimeError("resolver reset mid-run: unexpected second first collect")
            state.saw_first = True
            state.new_infos += len(infos)
            if state.sigs:
                # Baseline for every first-seal entry: the next collect that
                # re-walks one of these compares against this appearance.
                compare(state, infos, set(), set())
        return result

    def ctor_blob(info):
        t0 = time.perf_counter()
        try:
            return orig_ctor_blob(info)
        finally:
            elapsed = time.perf_counter() - t0
            if state.in_first_build:
                state.ctor_blob_first_s += elapsed
            else:
                state.ctor_blob_s += elapsed

    mb.BuildManager._build_native_resolvers = build
    mb.BuildManager._collect_incremental = collect
    mb._split_native_pending = split
    mb._native_ctor_blob = ctor_blob


def report(state: State, run_status: str) -> None:
    def out(name: str, value) -> None:
        print(f"[snap] {name:34s} {value}", file=sys.stderr)

    update_installs = (
        state.build_s
        - state.first_build_s
        - state.split_s
        - state.ctor_blob_s
        - (state.collect_s - state.first_collect_s)
        - state.sig_in_build_s
    )
    out("run status", run_status)
    out("sigs mode", state.sigs)
    out("collects", state.collects)
    out("first collects (full build)", state.first_collects)
    out("empty-scc collects", state.empty_scc_collects)
    out("collected infos (sum)", state.collected_infos)
    out("collected aliases (sum)", state.collected_aliases)
    out("new infos (sum)", state.new_infos)
    out("re-collected (already snapshotted)", state.recollected)
    out("re-collected builtins", state.recollected_builtins)
    out("re-push feed to Rust (sum)", state.repush)
    out("resolver build_s", f"{state.build_s:.3f}")
    out("  first full-build_s", f"{state.first_build_s:.3f}")
    out("  first collect walk_s", f"{state.first_collect_s:.3f}")
    out("  first ctor blob_s", f"{state.ctor_blob_first_s:.3f}")
    out("  collect walk_s (later)", f"{state.collect_s - state.first_collect_s:.3f}")
    out("  sig gate split_s", f"{state.split_s:.3f}")
    out("  ctor blob_s", f"{state.ctor_blob_s:.3f}")
    out("  probe sig in-build_s", f"{state.sig_in_build_s:.3f}")
    out("  update+installs residual_s", f"{update_installs:.3f}")
    out("write_str during resolver build", state.build_write_str)
    out("write_str total", state.total_write_str)
    out("probe sig overhead_s", f"{state.sig_s:.3f}")
    out("probe mirror failures", state.mirror_failures)
    if state.sigs:
        frac = state.unchanged / state.comparisons if state.comparisons else None
        rfrac = (
            state.repush_unchanged / state.repush_comparisons if state.repush_comparisons else None
        )
        out("sig comparisons", state.comparisons)
        out("sig unchanged", state.unchanged)
        out("unchanged fraction", f"{frac:.4f}" if frac is not None else "n/a")
        out("re-collected comparisons", state.recollected_comparisons)
        out("re-collected unchanged", state.recollected_unchanged)
        out("repush comparisons", state.repush_comparisons)
        out("repush unchanged", state.repush_unchanged)
        out(
            "repush unchanged fraction",
            f"{rfrac:.4f}" if rfrac is not None else "n/a (no repush compared)",
        )
        out("repush bytes (mirror proxy)", state.repush_bytes)
        out("repush unchanged bytes", state.repush_unchanged_bytes)
        out("no-baseline appearances", state.no_baseline)
        out("changed re-collected entries", len(state.changed_entries))
        for e in state.changed_entries:
            out(
                "  changed",
                f"collect={e.collect_idx} reason={e.reason} module={e.module} "
                f"repushed={e.repushed} bytes {e.prev_len}->{e.sig_len} {e.fullname}",
            )
            for f_name, prev_n, cur_n in e.fields:
                out("    field", f"{f_name} {prev_n}->{cur_n} bytes")


def evidence_refusals(state: State, run_status: str) -> list[str]:
    reasons = []
    if not run_status.startswith("completed"):
        reasons.append(f"run status {run_status!r} is not a completed check")
    if state.collects < 100:
        reasons.append(f"hollow profile: only {state.collects} collects")
    if state.new_infos == 0:
        reasons.append("hollow profile: no new infos (seam never engaged)")
    if state.total_write_str == 0:
        reasons.append("hollow profile: write_str counter never fired")
    if state.sigs and state.comparisons == 0:
        reasons.append("hollow profile: no signature comparisons")
    if state.mirror_failures:
        reasons.append(f"{state.mirror_failures} signature-mirror failures")
    if state.sigs and any(
        not e.reason or "not-walked" in e.reason or "no-manager" in e.reason
        for e in state.changed_entries
    ):
        reasons.append("changed entry with no walk reason (probe defect)")
    return reasons


def main() -> int:
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if root not in sys.path:
        sys.path.insert(0, root)
    for finder in list(sys.meta_path):
        if "editable" in (type(finder).__module__ or "") or "editable" in repr(finder):
            sys.meta_path.remove(finder)
    argv = list(sys.argv[1:])
    sigs = False
    while argv and argv[0].startswith("--") and argv[0] != "--":
        if argv[0] == "--sigs":
            sigs = True
            argv.pop(0)
        else:
            print(f"[snap] unknown flag {argv[0]}", file=sys.stderr)
            return 2
    if "--" in argv:
        rest = argv[argv.index("--") + 1 :]
    else:
        rest = argv
    argv = rest or list(DEFAULT_ARGS)
    os.environ["MYPY_NUM_WORKERS"] = "0"
    import mypy

    if not os.path.abspath(mypy.__file__).startswith(root + os.sep):
        print(f"[snap] mypy resolves outside this tree: {mypy.__file__}", file=sys.stderr)
        return 1
    state = State(sigs=sigs)
    install(state)
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
        # Operator action, not the run's own failure (#1846).
        raise
    except BaseException as exc:
        run_status = f"FAILED, raised {type(exc).__name__}: {exc}"
        import traceback

        traceback.print_exc()
    report(state, run_status)
    refusals = evidence_refusals(state, run_status)
    for reason in refusals:
        print(f"[snap] EVIDENCE REFUSED: {reason}", file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main())
