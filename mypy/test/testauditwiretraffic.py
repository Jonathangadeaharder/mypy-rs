"""Guards for `misc/audit_wire_traffic.py` (#1820, #1821, #1827, #1828, #1831,
#1837, #1846).

The audit counts seam calls with an in-process proxy, so a check that fans
out to workers makes it report 0 calls for every seam and still succeed.
Two guards close that: a fan-out refusal before any work starts, and a
non-zero exit when the run counted nothing at all. A third refuses when
`type_kernel` itself is unimportable, the one cause the hollow-zero
message names that would otherwise die in a bare `ModuleNotFoundError`
before the guard could run (#1821).

Two more refusals were added after review: the audit now also fails when
the audited run aborted (a crashed run still produced a partial report and
exited 0 as long as one call was counted), and it names the extension
build behind the numbers (#1828). The refusal for the unimportable kernel
is headed from its cause, because `except ImportError` also catches a
failure inside `type_kernel`'s own import (#1831).

Two further defects in the same tool are guarded here. An operator Ctrl-C
used to be recorded as the audited run's failure, which then attributed the
abort to mypy and printed a census for a run the operator had just stopped;
the interrupt now leaves `main()` before the report (#1846). And a blob the
wire cache hands to a second call site used to credit that later site; it
keeps the row of the site that registered it first, with the
re-registrations counted and marked on the affected row (#1837).

`main()` is driven with the kernel probe and `mypy.main.main` stubbed out,
so the guards are proven without a self-check. A guard that rejected every
run would be as useless as the hollow zero, so each refusal is paired with
a positive control that must return 0.

The probes' decision classification is guarded here too (#36): the coded
single-pair seam (1/0/3 decided, -1 deferred, #33) and the batch slot
seam are classified by both the audit's seam wrapper and the share probe's
counting proxy, and a misclassification silently mis-splits useful vs
deferred bytes and calls. The share probe's display is guarded alongside:
a 99.72% native seam must not print as "100%", and its deferral list must
rank by deferral count, not call count. The shares divide by the decision
total, not the call count, so a batch seam answers many slots per call
without printing over-100% rows (#41).
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import sys
import tempfile
import types
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

import mypy.main

_AUDIT_PATH = Path(__file__).resolve().parents[2] / "misc" / "audit_wire_traffic.py"
_SHARE_PATH = Path(__file__).resolve().parents[2] / "scripts" / "measure_native_share.py"


def _load_audit_module() -> types.ModuleType:
    """A fresh module per test: the counters are module-level globals."""
    spec = importlib.util.spec_from_file_location("audit_wire_traffic_under_test", _AUDIT_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_share_module() -> types.ModuleType:
    """A fresh `scripts/measure_native_share.py`, same loading contract."""
    spec = importlib.util.spec_from_file_location("measure_native_share_under_test", _SHARE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class AuditGuardHarness(unittest.TestCase):
    """Drive the audit's `main()` with the kernel and mypy stubbed out."""

    def setUp(self) -> None:
        self.audit = _load_audit_module()
        self.touched: list[str] = []
        self.argv_before: list[str] = []
        self.argv_after: list[str] = []
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def config_file(self, num_workers: int) -> str:
        path = Path(self.tmp.name) / "audit.ini"
        path.write_text(f"[mypy]\nnum_workers = {num_workers}\n")
        return str(path)

    def run_main(
        self,
        args: str,
        tally: int = 0,
        workers_env: str | None = None,
        run_exc: BaseException | None = None,
        stderr: io.StringIO | None = None,
    ) -> tuple[int, str]:
        """Run `main()` once.

        `tally` is the seam calls the stub run "made"; `run_exc` is raised by
        the stub in place of returning, which is how a crashed audited run is
        driven (#1828). `stderr` is for the callers that drive a `main()` which
        does not return (#1846), where the returned text never arrives.
        """

        def kernel_stub() -> int:
            self.touched.append("patch_kernel")
            return 3

        def serializer_stub() -> int:
            self.touched.append("patch_serializers")
            return 0

        def probe_stub() -> int:
            self.touched.append("patch_probes")
            return 0

        def mypy_stub(*args_: Any, **kwargs: Any) -> None:
            self.touched.append("mypy.main.main")
            if tally:
                self.audit.seam_calls["rust_stub_probe"] += tally
            if run_exc is not None:
                raise run_exc

        err = stderr if stderr is not None else io.StringIO()
        with mock.patch.dict(os.environ, clear=False):
            for key in ("MYPY_AUDIT_ARGS", "MYPY_NUM_WORKERS"):
                os.environ.pop(key, None)
            os.environ["MYPY_AUDIT_ARGS"] = args
            if workers_env is not None:
                os.environ["MYPY_NUM_WORKERS"] = workers_env
            self.argv_before = list(sys.argv)
            with (
                mock.patch.object(self.audit, "patch_kernel", kernel_stub),
                mock.patch.object(self.audit, "patch_serializers", serializer_stub),
                mock.patch.object(self.audit, "patch_probes", probe_stub),
                mock.patch.object(mypy.main, "main", mypy_stub),
                mock.patch.object(sys, "argv", list(sys.argv)),
                contextlib.redirect_stderr(err),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                rc = self.audit.main()
                self.argv_after = list(sys.argv)
        return rc, err.getvalue()


class FanoutGuardSuite(AuditGuardHarness):
    def test_explicit_n4_refuses_before_any_work(self) -> None:
        rc, err = self.run_main("-n4 --no-incremental -p mypy")
        self.assertEqual(rc, 1)
        self.assertIn("REFUSING TO RUN", err)
        self.assertIn("num_workers = 4", err)
        self.assertIn("'-n4'", err)
        self.assertIn("-n0", err)
        # The refusal precedes the probe and the run: nothing was patched,
        # mypy was never entered, and no report was printed.
        self.assertEqual(self.touched, [])
        self.assertNotIn("wire-waste audit", err)
        # And the argv is never rewritten to hide the caller's mistake.
        self.assertEqual(self.argv_after, self.argv_before)

    def test_every_fanout_spelling_refuses(self) -> None:
        for args in (
            "-n4 -p mypy",
            "-n 4 -p mypy",
            "--num-workers 2 -p mypy",
            "--num-workers=3 -p mypy",
            "-n1 -p mypy",
        ):
            with self.subTest(args=args):
                self.touched.clear()
                rc, err = self.run_main(args)
                self.assertEqual(rc, 1, f"{args}: {err}")
                self.assertIn("REFUSING TO RUN", err)
                self.assertEqual(self.touched, [])

    def test_parallel_config_without_an_override_refuses(self) -> None:
        cfg = self.config_file(4)
        rc, err = self.run_main(f"-p mypy --config-file {cfg}")
        self.assertEqual(rc, 1)
        self.assertIn("num_workers = 4", err)
        self.assertIn(cfg, err)
        self.assertEqual(self.touched, [])

    def test_parallel_env_without_an_override_refuses(self) -> None:
        # MYPY_NUM_WORKERS alone fans the run out (`mypy/main.py`).
        rc, err = self.run_main(f"-p mypy --config-file {self.config_file(0)}", workers_env="4")
        self.assertEqual(rc, 1)
        self.assertIn("MYPY_NUM_WORKERS", err)
        self.assertIn("num_workers = 4", err)
        self.assertEqual(self.touched, [])

    def test_unparseable_worker_value_refuses(self) -> None:
        rc, err = self.run_main(f"-n foo -p mypy --config-file {self.config_file(0)}")
        self.assertEqual(rc, 1)
        self.assertIn("not an integer", err)
        self.assertEqual(self.touched, [])

    def test_explicit_n0_beats_a_parallel_config(self) -> None:
        # Positive control for the config path: the same config that refuses
        # above is accepted once the command line pins -n0.
        cfg = self.config_file(4)
        rc, err = self.run_main(f"-n0 -p mypy --config-file {cfg}", tally=1)
        self.assertEqual(rc, 0, err)
        self.assertNotIn("REFUSING TO RUN", err)
        self.assertNotIn("HOLLOW ZERO", err)
        self.assertIn("wire-waste audit", err)
        # The argv that ran is the argv that was recorded: the caller's own
        # `--config-file` and `-n0` survive verbatim, nothing is rewritten.
        self.assertEqual(
            self.argv_after,
            [
                "mypy",
                "--config-file",
                "mypy_self_check.ini",
                "-n0",
                "-p",
                "mypy",
                "--config-file",
                cfg,
            ],
        )

    def test_last_worker_flag_wins(self) -> None:
        # argparse keeps the last value, so the guard must too.
        cfg = self.config_file(0)
        rc, err = self.run_main(f"-n4 -n0 -p mypy --config-file {cfg}", tally=1)
        self.assertEqual(rc, 0, err)
        self.assertNotIn("REFUSING TO RUN", err)

    def test_default_invocation_is_accepted_and_runs(self) -> None:
        rc, err = self.run_main("", tally=1)
        self.assertEqual(rc, 0, err)
        # Non-empty first: a guard regression must not read as an IndexError.
        self.assertTrue(self.touched, f"nothing ran: {err}")
        self.assertEqual(self.touched[0], "patch_kernel")
        self.assertIn("mypy.main.main", self.touched)


class HollowZeroGuardSuite(AuditGuardHarness):
    def test_zero_calls_fail_after_the_report_is_printed(self) -> None:
        cfg = self.config_file(0)
        rc, err = self.run_main(f"-n0 -p mypy --config-file {cfg}", tally=0)
        self.assertEqual(rc, 1)
        self.assertEqual(
            self.touched, ["patch_kernel", "patch_serializers", "patch_probes", "mypy.main.main"]
        )
        # The partial report is printed, then the run fails: the numbers stay
        # visible, but they cannot be read as evidence.
        self.assertIn("wire-waste audit", err)
        self.assertIn("HOLLOW ZERO", err)
        self.assertLess(err.index("wire-waste audit"), err.index("HOLLOW ZERO"))
        self.assertIn("0 calls across 0 registered seam(s)", err)
        self.assertIn("-n0", err)

    def test_a_raising_run_still_reads_the_tally(self) -> None:
        # Accounting must not be bypassed on the abnormal path: the audited
        # run raising must not skip the guard.
        cfg = self.config_file(0)

        def raising_stub(*args_: Any, **kwargs: Any) -> None:
            self.touched.append("mypy.main.main")
            raise RuntimeError("audited run died")

        err = io.StringIO()
        with mock.patch.dict(os.environ, clear=False):
            for key in ("MYPY_AUDIT_ARGS", "MYPY_NUM_WORKERS"):
                os.environ.pop(key, None)
            os.environ["MYPY_AUDIT_ARGS"] = f"-n0 -p mypy --config-file {cfg}"
            with (
                mock.patch.object(self.audit, "patch_kernel", lambda: 0),
                mock.patch.object(self.audit, "patch_serializers", lambda: 0),
                mock.patch.object(self.audit, "patch_probes", lambda: 0),
                mock.patch.object(mypy.main, "main", raising_stub),
                mock.patch.object(sys, "argv", list(sys.argv)),
                contextlib.redirect_stderr(err),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                rc = self.audit.main()
        output = err.getvalue()
        self.assertEqual(rc, 1)
        self.assertIn("RuntimeError: audited run died", output)
        self.assertIn("HOLLOW ZERO", output)


class CrashedRunGuardSuite(AuditGuardHarness):
    """#1828: the exit status must never come from the tally alone."""

    def test_a_run_that_exits_2_with_counted_calls_refuses(self) -> None:
        # The issue's own probe: a stale kernel crashes mypy with
        # `INTERNAL ERROR` and exit 2 after counting seam calls, and the audit
        # used to return 0 with a truncated report.
        rc, err = self.run_main("", tally=3, run_exc=SystemExit(2))
        self.assertEqual(rc, 1, err)
        self.assertIn("THE AUDITED RUN FAILED (#1828)", err)
        self.assertIn("failure: mypy exited 2", err)
        # The partial numbers are still worth reading, so the report is
        # printed first and the refusal follows it.
        self.assertIn("wire-waste audit", err)
        self.assertLess(err.index("wire-waste audit"), err.index("THE AUDITED RUN FAILED"))
        # The report itself carries the outcome: a reader who only sees the
        # report must not read a truncated census as a complete one.
        self.assertIn("audited run: FAILED, mypy exited 2", err)

    def test_a_raising_run_with_counted_calls_refuses(self) -> None:
        # The same defect for any other abort, not just mypy's own exit path.
        rc, err = self.run_main("", tally=3, run_exc=RuntimeError("stale kernel blew up"))
        self.assertEqual(rc, 1)
        self.assertIn("failure: RuntimeError: stale kernel blew up", err)
        self.assertIn("audited run: FAILED, raised RuntimeError: stale kernel blew up", err)

    def test_a_crashed_run_with_no_calls_reports_both_causes(self) -> None:
        # A run that aborted before counting anything is both defects; one
        # message must not hide the other.
        rc, err = self.run_main("", tally=0, run_exc=SystemExit(2))
        self.assertEqual(rc, 1)
        self.assertIn("THE AUDITED RUN FAILED (#1828)", err)
        self.assertIn("HOLLOW ZERO", err)
        self.assertLess(err.index("THE AUDITED RUN FAILED"), err.index("HOLLOW ZERO"))

    def test_a_clean_run_with_counted_calls_is_accepted(self) -> None:
        # Positive control: `clean_exit=True` makes mypy return normally on a
        # clean run, which must stay a readable report.
        rc, err = self.run_main("", tally=1)
        self.assertEqual(rc, 0, err)
        self.assertIn("audited run: completed, mypy.main returned", err)
        # The header names the extension build the numbers came from (#1828),
        # so a stale `.so` cannot read like a rebuilt one.
        self.assertIn("type_kernel:", err)
        self.assertIn("ast_serialize:", err)
        self.assertIn("module_resolver:", err)

    def test_a_run_that_found_type_errors_is_accepted(self) -> None:
        # Positive control: mypy exits 1 when it found type errors, having
        # still checked every module, so the census is complete.
        rc, err = self.run_main("", tally=1, run_exc=SystemExit(1))
        self.assertEqual(rc, 0, err)
        self.assertIn("audited run: completed, mypy exit 1", err)
        self.assertNotIn("THE AUDITED RUN FAILED", err)

    def test_a_zero_exit_with_counted_calls_is_accepted(self) -> None:
        rc, err = self.run_main("", tally=1, run_exc=SystemExit(0))
        self.assertEqual(rc, 0, err)
        self.assertIn("audited run: completed, mypy exit 0", err)


class OperatorAbortSuite(AuditGuardHarness):
    """#1846: a Ctrl-C is an operator action, not the audited run's failure."""

    def abort(self, tally: int = 3) -> str:
        """Drive `main()` under a simulated Ctrl-C, returning its output."""
        err = io.StringIO()
        with self.assertRaises(KeyboardInterrupt):
            self.run_main("", tally=tally, run_exc=KeyboardInterrupt(), stderr=err)
        return err.getvalue()

    def test_an_operator_interrupt_aborts_without_a_census(self) -> None:
        output = self.abort()
        # The census is the work the interrupt asked to stop: it walks
        # `pending` and `dedup_hit_blobs`, so it must not run at all. A test
        # reading only the message would not tell the two designs apart.
        self.assertNotIn("wire-waste audit", output)
        # And the abort is not the audited run's failure: recording it as one
        # printed `FAILED, raised KeyboardInterrupt`, a crash-shaped traceback,
        # and fired the run-failure refusal for an operator action.
        self.assertNotIn("FAILED, raised KeyboardInterrupt", output)
        self.assertNotIn("Traceback (most recent call last)", output)
        self.assertNotIn("THE AUDITED RUN FAILED", output)
        self.assertIn("ABORTED by the operator (#1846)", output)

    def test_the_abort_notice_carries_the_tally_so_far(self) -> None:
        # The notice replaces the census, so it must say how far the run got
        # rather than leaving the operator with nothing.
        self.assertIn("seam calls counted before the interrupt: 3", self.abort(tally=3))

    def test_the_abort_notice_reads_the_tally_not_a_constant(self) -> None:
        # A second test method, so a fresh module: the number must come from
        # the counter, not from a leftover or a literal.
        self.assertIn("seam calls counted before the interrupt: 5", self.abort(tally=5))


class _MissingTypeKernel:
    """Meta-path finder that makes `import type_kernel` fail, nothing else."""

    def find_spec(self, name: str, path: Any = None, target: Any = None) -> None:
        if name == "type_kernel":
            raise ModuleNotFoundError(f"No module named {name!r}", name=name)
        return


class MissingKernelGuardSuite(AuditGuardHarness):
    """The real `patch_kernel` against an unimportable extension (#1821)."""

    def run_refusal(self) -> tuple[int, str]:
        """Drive the real `patch_kernel` with `import type_kernel` failing."""
        # Unlike the other suites, `patch_kernel` is NOT stubbed: the real
        # import runs and fails, which is the defect #1821 reports.
        cfg = self.config_file(0)

        def serializer_stub() -> int:
            self.touched.append("patch_serializers")
            return 0

        def probe_stub() -> int:
            self.touched.append("patch_probes")
            return 0

        def mypy_stub(*args_: Any, **kwargs: Any) -> None:
            self.touched.append("mypy.main.main")

        err = io.StringIO()
        with mock.patch.dict(os.environ, clear=False):
            for key in ("MYPY_AUDIT_ARGS", "MYPY_NUM_WORKERS"):
                os.environ.pop(key, None)
            os.environ["MYPY_AUDIT_ARGS"] = f"-n0 -p mypy --config-file {cfg}"
            with (
                mock.patch.object(self.audit, "patch_serializers", serializer_stub),
                mock.patch.object(self.audit, "patch_probes", probe_stub),
                mock.patch.object(mypy.main, "main", mypy_stub),
                # `import type_kernel` reads `sys.modules` before any meta-path
                # finder runs, so the cache is emptied alongside the finder
                # (#1831); a cached module would never consult the finder.
                mock.patch.dict(sys.modules),
                mock.patch.object(sys, "meta_path", [_MissingTypeKernel(), *sys.meta_path]),
                mock.patch.object(sys, "argv", list(sys.argv)),
                contextlib.redirect_stderr(err),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                sys.modules.pop("type_kernel", None)
                rc = self.audit.main()
        return rc, err.getvalue()

    def test_missing_kernel_refuses_with_the_remedy_before_any_work(self) -> None:
        rc, output = self.run_refusal()
        # Fail loud: non-zero, a named refusal, and the remedy the bare
        # `ModuleNotFoundError` never carried.
        self.assertEqual(rc, 1, output)
        self.assertIn("MISSING type_kernel EXTENSION (#1821)", output)
        self.assertIn("cause: ModuleNotFoundError: No module named 'type_kernel'", output)
        self.assertIn(
            "/private/tmp/mypy-rs-<lane>-tk"
            ":/private/tmp/mypy-rs-local-ast:/private/tmp/mypy-rs-local-resolver",
            output,
        )
        # The stale shared-venv stub is why the remedy is not "just run it".
        self.assertIn("#1800", output)
        # A refusal, not a hollow-zero report: nothing ran, nothing is
        # registered, and no report was printed.
        self.assertEqual(self.touched, [])
        self.assertEqual(self.audit.registered_seams, set())
        self.assertNotIn("wire-waste audit", output)

    def test_a_cached_type_kernel_does_not_defeat_the_refusal(self) -> None:
        # The control for that neutralisation: a fake kernel with a
        # real-looking seam is cached, so the finder is consulted only if the
        # cache is emptied -- in any environment, parity or not.
        fake = types.ModuleType("type_kernel")
        fake.rust_fake_seam = lambda: None  # type: ignore[attr-defined]
        with mock.patch.dict(sys.modules, {"type_kernel": fake}):
            self.assertIs(sys.modules["type_kernel"], fake)
            rc, output = self.run_refusal()
        self.assertEqual(rc, 1, output)
        self.assertIn("MISSING type_kernel EXTENSION (#1821)", output)
        self.assertEqual(self.audit.registered_seams, set())
        self.assertNotIn("wire-waste audit", output)


class GuardPolicySuite(unittest.TestCase):
    """The policies called directly: no `main()`, no kernel, no run."""

    def setUp(self) -> None:
        self.audit = _load_audit_module()
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def config_body(self, body: str, name: str = "audit.ini") -> str:
        path = Path(self.tmp.name) / name
        path.write_text(body)
        return str(path)

    def test_zero_worker_spellings_are_accepted(self) -> None:
        cfg = self.config_body("[mypy]\nnum_workers = 0\n")
        for extra in (["-n0"], ["-n", "0"], ["--num-workers", "0"], ["--num-workers=0"]):
            with self.subTest(extra=extra):
                argv = ["mypy", "--config-file", cfg, *extra]
                self.assertIsNone(self.audit.fanout_reason(argv, {"MYPY_NUM_WORKERS": "0"}))

    def test_absent_config_key_is_not_a_fanout(self) -> None:
        cfg = self.config_body("[mypy]\nstrict = True\n")
        self.assertIsNone(self.audit.fanout_reason(["mypy", "--config-file", cfg], {}))

    def test_argv_wins_over_env_and_config(self) -> None:
        cfg = self.config_body("[mypy]\nnum_workers = 4\n")
        argv = ["mypy", "--config-file", cfg, "-n0", "-p", "mypy"]
        self.assertIsNone(self.audit.fanout_reason(argv, {"MYPY_NUM_WORKERS": "4"}))

    def test_config_path_comes_from_argv(self) -> None:
        # The guard reads the config mypy will read, not a hardcoded name.
        parallel = self.config_body("[mypy]\nnum_workers = 2\n", "parallel.ini")
        sequential = self.config_body("[mypy]\nnum_workers = 0\n", "sequential.ini")
        self.assertIsNone(self.audit.fanout_reason(["mypy", "--config-file", sequential], {}))
        reason = self.audit.fanout_reason(["mypy", "--config-file", parallel], {})
        assert reason is not None
        self.assertIn(parallel, reason)
        self.assertIn("num_workers = 2", reason)

    def test_missing_config_is_left_to_mypy(self) -> None:
        # mypy errors on a missing --config-file itself; the guard must not
        # invent a fan-out it cannot see.
        missing = str(Path(self.tmp.name) / "absent.ini")
        self.assertIsNone(self.audit.fanout_reason(["mypy", "--config-file", missing], {}))

    def test_hollow_zero_reason_reads_the_tally(self) -> None:
        self.assertIsNone(self.audit.hollow_zero_reason(1, 12))
        self.assertIsNone(self.audit.hollow_zero_reason(17_584, 155))
        message = self.audit.hollow_zero_reason(0, 12)
        assert message is not None
        self.assertIn("0 calls across 12 registered seam(s)", message)
        self.assertIn("PYTHONPATH", message)
        self.assertIn("-n0", message)

    def test_pinned_invocation_keeps_n0(self) -> None:
        # The docstring's `-n0` is load-bearing (#1820), so it stays pinned.
        self.assertIn("-n0", self.audit.audit_argv(None))
        self.assertEqual(self.audit.audit_argv("-n0 -p mypy")[3:], ["-n0", "-p", "mypy"])
        self.assertEqual(self.audit.audit_argv("-p mypy")[3:], ["-p", "mypy"])

    def test_missing_kernel_reason_keeps_a_different_cause(self) -> None:
        # A transitive ImportError must stay diagnosable: the message carries
        # the cause verbatim rather than flattening it to "no type_kernel".
        message = self.audit.missing_kernel_reason(
            ModuleNotFoundError("No module named 'librt'", name="librt")
        )
        self.assertIn("cause: ModuleNotFoundError: No module named 'librt'", message)
        self.assertIn("type_kernel", message)

    def test_missing_kernel_headline_follows_the_cause(self) -> None:
        # #1831: in the librt case the extension is present and only its own
        # import failed, so the headline must not claim it is missing.
        dependency = self.audit.missing_kernel_reason(
            ModuleNotFoundError("No module named 'librt'", name="librt")
        )
        self.assertIn("type_kernel UNAVAILABLE (#1821)", dependency)
        self.assertNotIn("MISSING type_kernel EXTENSION", dependency)
        extension = self.audit.missing_kernel_reason(
            ModuleNotFoundError("No module named 'type_kernel'", name="type_kernel")
        )
        self.assertIn("MISSING type_kernel EXTENSION (#1821)", extension)
        # A bare ImportError (no `name`) is a failed initialisation, not a
        # missing file: mypy's Rust init errors carry no `name`.
        bare = self.audit.missing_kernel_reason(ImportError("librt symbol missing"))
        self.assertIn("type_kernel UNAVAILABLE (#1821)", bare)

    def test_run_exit_codes_name_a_failure(self) -> None:
        # #1828: only an exit that truncates the census is a failure. 0 is a
        # clean run, 1 means type errors were found after every module was
        # checked, 2 is a serious error or a build stopped by blockers.
        self.assertIsNone(self.audit.classify_run_exit(None))
        self.assertIsNone(self.audit.classify_run_exit(0))
        self.assertIsNone(self.audit.classify_run_exit(1))
        self.assertEqual(self.audit.classify_run_exit(2), "mypy exited 2")
        self.assertEqual(self.audit.classify_run_exit("boom"), "mypy exited 'boom'")

    def test_evidence_refusals_collect_every_reason(self) -> None:
        # A run that crashed before counting anything is both defects, so both
        # messages print: either alone would hide a real cause.
        both = self.audit.evidence_refusals(0, 3, "mypy exited 2")
        self.assertEqual(len(both), 2)
        self.assertIn("THE AUDITED RUN FAILED (#1828)", both[0])
        self.assertIn("HOLLOW ZERO", both[1])
        # A readable report, and one refusal at a time.
        self.assertEqual(self.audit.evidence_refusals(1, 3, None), [])
        self.assertEqual(len(self.audit.evidence_refusals(1, 3, "mypy exited 2")), 1)
        self.assertEqual(len(self.audit.evidence_refusals(0, 3, None)), 1)

    def test_extension_identity_names_the_build(self) -> None:
        # #1828: the header must answer "which build produced these numbers".
        import mypy

        lines = "\n".join(self.audit.extension_identity(mypy))
        for name in ("type_kernel", "ast_serialize", "module_resolver"):
            self.assertIn(name, lines)
        self.assertIn("crates/type_kernel/src", lines)


class UnconsumedCauseSplitSuite(unittest.TestCase):
    """#1827: the unconsumed bucket's cause split, and the probe behind it."""

    def setUp(self) -> None:
        self.audit = _load_audit_module()

    def report_text(self) -> str:
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            self.audit.report("completed, mypy.main returned")
        return err.getvalue()

    def seed_pending(self, caller: str, n: int, payload: int = 100, turned_away: int = 0) -> None:
        """Register `n` unconsumed blobs in `pending`, as a run leaves them.

        The first `turned_away` of them are also recorded as pair keys a dedup
        hit turned away, which is the identity the report joins on.
        """
        for i in range(n):
            blob = f"{caller}-{i}".encode()
            self.audit.pending[id(blob)] = [caller, payload, blob]
            if i < turned_away:
                self.audit.dedup_hit_blobs[id(blob)] = blob

    def test_the_report_names_the_dominant_site_and_the_dedup_share(self) -> None:
        self.seed_pending("subtypes.py:879:_is_subtype", 30, turned_away=10)
        self.seed_pending("subtypes.py:880:_is_subtype", 10)
        self.audit.subtype_dedup.update({"probes": 90, "hits": 10})
        # `mock.patch.object`, not a bare assignment: this module is loaded
        # dynamically, so mypy sees only `ModuleType` and rejects a direct
        # attribute write (`mypy/test/*` is inside the self-check's `-p mypy`).
        with mock.patch.object(self.audit, "subtype_dedup_status", "installed"):
            output = self.report_text()
        self.assertIn("--- B2. unconsumed bucket by cause (#1827) ---", output)
        self.assertIn(
            "dominant call site: subtypes.py:879:_is_subtype 30 events, "
            "3000B (75.0% of the bucket)",
            output,
        )
        self.assertIn("10 dedup hits of 90 lookups", output)
        self.assertIn("a dedup hit turned these away: 10 of the 40 unconsumed events", output)
        self.assertIn("(25.0%, 1000 B)", output)
        self.assertIn("other causes: 30 events", output)
        # The reduction path is recorded too: the key IS the bytes, so no
        # reordering removes them (#1827 fix direction 2).
        self.assertIn("reduction path: the cache key is the serialization", output)

    def test_the_split_joins_by_identity_not_by_the_hit_count(self) -> None:
        # Two blobs turned away against a hit count of 30121: the split follows
        # the blob identities, so it cannot swallow the non-dedup remainder by
        # extrapolating two events per hit (#1827).
        self.seed_pending("subtypes.py:879:_is_subtype", 30, turned_away=2)
        self.seed_pending("subtypes.py:880:_is_subtype", 10)
        self.audit.subtype_dedup.update({"probes": 90_000, "hits": 30_121})
        with mock.patch.object(self.audit, "subtype_dedup_status", "installed"):
            output = self.report_text()
        self.assertIn("a dedup hit turned these away: 2 of the 40 unconsumed events", output)
        self.assertIn("other causes: 38 events", output)

    def test_a_bucket_the_probe_fully_explains_has_no_other_causes(self) -> None:
        self.seed_pending("subtypes.py:879:_is_subtype", 5, turned_away=5)
        self.audit.subtype_dedup.update({"probes": 5, "hits": 5})
        with mock.patch.object(self.audit, "subtype_dedup_status", "installed"):
            output = self.report_text()
        self.assertIn("a dedup hit turned these away: 5 of the 5 unconsumed events", output)
        self.assertNotIn("other causes:", output)

    def test_an_uninstalled_probe_is_not_reported_as_zero_hits(self) -> None:
        # A structural zero would read as "no dedup hits"; the report must say
        # the measurement is missing instead (#1827, AGENTS.md).
        self.seed_pending("subtypes.py:879:_is_subtype", 3, turned_away=3)
        with mock.patch.object(self.audit, "subtype_dedup_status", "NOT INSTALLED: no dict"):
            output = self.report_text()
        self.assertIn("subtype dedup probe: NOT INSTALLED: no dict", output)
        self.assertNotIn("dedup hits of", output)
        self.assertNotIn("turned these away", output)
        self.assertNotIn("reduction path:", output)
        # The dominant site stays visible either way.
        self.assertIn("dominant call site:", output)

    def test_counting_answers_counts_lookups_and_hits(self) -> None:
        answers = self.audit._CountingAnswers({(b"l", b"r", ()): True})
        self.assertIn((b"l", b"r", ()), answers)
        self.assertNotIn((b"x", b"y", ()), answers)
        self.assertEqual(self.audit.subtype_dedup, {"probes": 2, "hits": 1})

    def test_a_build_boundary_reset_keeps_the_counter_installed(self) -> None:
        # The reset reassigns `_subtype_answers`, dropping its contents by
        # design; the counter must survive it, or the report would read as
        # "no dedup hits" for the rest of the run.
        holder = types.SimpleNamespace(
            _subtype_answers=self.audit._CountingAnswers({(b"k",): True})
        )

        def reset() -> None:
            holder._subtype_answers = {}

        self.audit._reinstall_after_reset(holder, reset)()
        self.assertIsInstance(holder._subtype_answers, self.audit._CountingAnswers)
        self.assertNotIn((b"k",), holder._subtype_answers)
        self.assertEqual(self.audit.subtype_dedup, {"probes": 1, "hits": 0})

    def test_the_probe_refuses_loudly_when_it_cannot_install(self) -> None:
        # A renamed or non-dict cache must be a refusal, not a silent zero.
        holder = types.SimpleNamespace(_subtype_answers=None)
        self.assertFalse(self.audit.patch_subtype_dedup_probe(holder))
        self.assertIn("NOT INSTALLED", self.audit.subtype_dedup_status)
        # A reset that is not a function would lose the counter mid-run.
        holder2 = types.SimpleNamespace(
            _subtype_answers={},
            _clear_subtype_batch=lambda: None,
            _set_native_subtype_active="not a function",
        )
        self.assertFalse(self.audit.patch_subtype_dedup_probe(holder2))
        self.assertIn("_set_native_subtype_active", self.audit.subtype_dedup_status)
        # Nothing was written before the refusal.
        self.assertNotIsInstance(holder2._subtype_answers, self.audit._CountingAnswers)

    def test_the_probe_installs_and_survives_a_reset(self) -> None:
        def reset() -> None:
            holder._subtype_answers = {}

        holder = types.SimpleNamespace(
            _subtype_answers={(b"a", b"b", ()): True},
            _clear_subtype_batch=reset,
            _set_native_subtype_active=lambda active: None,
        )
        self.assertTrue(self.audit.patch_subtype_dedup_probe(holder))
        self.assertEqual(self.audit.subtype_dedup_status, "installed")
        self.assertIsInstance(holder._subtype_answers, self.audit._CountingAnswers)
        # The live cache is wrapped, not replaced: its contents survive.
        self.assertIn((b"a", b"b", ()), holder._subtype_answers)
        # A build-boundary reset wipes the cache and reassigns the name; the
        # counter must come back with the new dict.
        holder._clear_subtype_batch()
        self.assertIsInstance(holder._subtype_answers, self.audit._CountingAnswers)
        self.assertNotIn((b"r", b"s", ()), holder._subtype_answers)
        self.assertEqual(self.audit.subtype_dedup, {"probes": 2, "hits": 1})

    def test_an_unreadable_batch_buffer_is_not_reported_as_zero(self) -> None:
        # The competing explanation for the bucket is a dropped batch buffer.
        # A buffer this probe cannot read must say so: printing "0 blobs" for
        # it would be a structural zero (#1827, AGENTS.md).
        self.seed_pending("mypy/subtypes.py:1:f", 1)
        # mypy.subtypes not imported: the share is unmeasured, not zero.
        with mock.patch.dict(sys.modules, {"mypy.subtypes": None}):
            self.assertIsNone(self.audit._buffered_blob_ids())
            self.assertIn("NOT READABLE", self.report_text())
        # An imported module without the attribute is the deleted-buffer
        # case (#33): the report must say the buffer is gone, not that
        # the share is unmeasured or zero.
        deleted = types.SimpleNamespace()
        with mock.patch.dict(sys.modules, {"mypy.subtypes": deleted}):
            self.assertIsNone(self.audit._buffered_blob_ids())
            self.assertIn("deleted (#33)", self.report_text())
            self.assertNotIn("NOT READABLE", self.report_text())
        # A readable but empty buffer is a real zero, and stays a number.
        empty = types.SimpleNamespace(_subtype_batch=[])
        with mock.patch.dict(sys.modules, {"mypy.subtypes": empty}):
            self.assertEqual(self.audit._buffered_blob_ids(), set())
            self.assertIn(
                "still in mypy.subtypes._subtype_batch at exit: 0 blobs", self.report_text()
            )


class SharedBlobAttributionSuite(unittest.TestCase):
    """#1837: a wire-cache-shared blob keeps the row of the site that built it.

    Every registration here goes through the real `register_serializer_result`
    rather than the seeded shapes the other suites use, so the mutation control
    for each assertion is a mutation of the attribution rule itself.
    """

    def setUp(self) -> None:
        self.audit = _load_audit_module()

    def register(self, caller: str, n: int, payload: int = 100) -> list[bytes]:
        """Register `n` distinct blobs of `payload` bytes from `caller`."""
        blobs = [f"{caller}#{i}".encode().ljust(payload, b".") for i in range(n)]
        for blob in blobs:
            self.audit.register_serializer_result(blob, caller)
        return blobs

    def report_text(self) -> str:
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            self.audit.report("completed, mypy.main returned")
        return err.getvalue()

    def test_the_first_registration_keeps_the_row(self) -> None:
        builder = "subtypes.py:879:_is_subtype"
        later = "subtypes.py:881:_is_subtype"
        blob = self.register(builder, 1)[0]
        self.audit.register_serializer_result(blob, later)
        # One event, keyed by identity, and it belongs to the site that had the
        # bytes built: the later site used to take the row (#1837).
        self.assertEqual(len(self.audit.pending), 1)
        self.assertEqual(self.audit.pending[id(blob)][0], builder)
        self.assertEqual(dict(self.audit.reregistered), {builder: 1})

    def test_a_same_site_re_registration_is_not_a_share(self) -> None:
        # A site can re-serialize a value it still has pending (a wire-cache
        # self-hit). The row is already that site's, and the report line says
        # "by another call site", so counting it as a share would mislabel it.
        builder = "subtypes.py:879:_is_subtype"
        blob = self.register(builder, 1)[0]
        self.audit.register_serializer_result(blob, builder)
        self.assertEqual(len(self.audit.pending), 1)
        self.assertEqual(dict(self.audit.reregistered), {})
        output = self.report_text()
        self.assertIn("wire-cache shares: 0 re-registration(s)", output)
        self.assertNotIn("[shared:", output.split("sections A, B and C")[1])

    def test_a_consumed_blob_is_not_a_re_registration(self) -> None:
        # The identity is already spent, so a wire-cache hit on it is not an
        # event at all: it must not resurrect a pending entry or inflate the
        # share count of a run that is over.
        builder = "types.py:5005:_restore_dedup_identity"
        blob = self.register(builder, 1)[0]
        self.audit.consume(blob, True, "rust_seam")
        self.audit.register_serializer_result(blob, "types.py:5100:_serialize_type")
        self.assertEqual(self.audit.pending, {})
        self.assertEqual(dict(self.audit.reregistered), {})

    def test_the_report_marks_the_shared_row_and_counts_the_shares(self) -> None:
        builder = "subtypes.py:879:_is_subtype"
        later = "subtypes.py:881:_is_subtype"
        blobs = self.register(builder, 4)
        self.register("subtypes.py:880:_is_subtype", 2)
        for blob in blobs[:2]:
            self.audit.register_serializer_result(blob, later)
        output = self.report_text()
        # The share is counted and the affected row says so, instead of the
        # row silently reading as that site's own cost.
        self.assertIn(
            "wire-cache shares: 2 re-registration(s) by another call site (#1837)", output
        )
        self.assertIn(f"  {4:8d}  {400:10d}B  {builder}  [shared: 2]", output)
        # The re-registering site paid nothing, so it gets no row of its own.
        self.assertNotIn(later, output)

    def test_a_report_without_sharing_says_zero_rather_than_nothing(self) -> None:
        # A structural zero: the line prints, so "no sharing" is measured
        # rather than silent, and no row carries a marker.
        self.register("subtypes.py:879:_is_subtype", 3)
        output = self.report_text()
        self.assertIn("wire-cache shares: 0 re-registration(s)", output)
        # The B row is compared whole: the header line carries the literal
        # "[shared: N]" as its legend, so a marker on the row is what this
        # asserts is absent.
        rows = [
            line
            for line in output.splitlines()
            if line.strip().endswith("subtypes.py:879:_is_subtype")
        ]
        self.assertEqual(rows, [f"  {3:8d}  {300:10d}B  subtypes.py:879:_is_subtype"])

    def test_sharing_does_not_move_the_dedup_split(self) -> None:
        # #1840's B2 joins the buckets against the dedup keys by blob identity,
        # so a shared blob keeps its place in the split: the credited caller
        # changed, the split's totals did not.
        blobs = self.register("subtypes.py:879:_is_subtype", 30)
        self.register("subtypes.py:880:_is_subtype", 10)
        for blob in blobs[:10]:
            self.audit.dedup_hit_blobs[id(blob)] = blob
        for blob in blobs[:5]:
            self.audit.register_serializer_result(blob, "subtypes.py:881:_is_subtype")
        self.audit.subtype_dedup.update({"probes": 90, "hits": 10})
        with mock.patch.object(self.audit, "subtype_dedup_status", "installed"):
            output = self.report_text()
        self.assertIn(
            "dominant call site: subtypes.py:879:_is_subtype 30 events, "
            "3000B (75.0% of the bucket)",
            output,
        )
        self.assertIn("a dedup hit turned these away: 10 of the 40 unconsumed events", output)
        self.assertIn("(25.0%, 1000 B)", output)
        self.assertIn("other causes: 30 events", output)
        self.assertIn("subtypes.py:879:_is_subtype  [shared: 5]", output)


class CodedSeamSplitSuite(unittest.TestCase):
    """#36: the probes' coded/batch classification, proven at the wrapper.

    Both probes classify `rust_is_subtype_coded` results (1/0/3 decided
    natively, -1 deferred, #33) and `rust_is_subtype_batch` slots, and a
    misclassification silently mis-splits useful vs deferred bytes and
    calls: the probes still run and still print numbers, so only a test
    can catch it (measurement code is evidence-critical, AGENTS.md).
    """

    def setUp(self) -> None:
        self.audit = _load_audit_module()
        self.share = _load_share_module()

    def test_audit_wrapper_splits_coded_codes_into_useful_and_deferred(self) -> None:
        codes = {"false": 0, "true": 1, "consult": 3, "defer": -1}
        lens: dict[str, int] = {}
        for label, code in codes.items():
            blob = f"blob-{label}".encode()
            self.audit.register_serializer_result(blob, f"site-{label}")
            lens[label] = len(blob)
            seam = self.audit.make_seam("rust_is_subtype_coded", lambda *a, c=code: c)
            seam(blob)
        self.assertEqual(dict(self.audit.seam_calls), {"rust_is_subtype_coded": 4})
        self.assertEqual(dict(self.audit.seam_defers), {"rust_is_subtype_coded": 1})
        # 1/0/3 are decided answers (0 is a decided False, 3 a consult
        # cut): useful bytes, never deferrals.
        self.assertEqual(
            dict(self.audit.useful), {"site-false": 1, "site-true": 1, "site-consult": 1}
        )
        self.assertEqual(
            dict(self.audit.useful_bytes),
            {
                "site-false": lens["false"],
                "site-true": lens["true"],
                "site-consult": lens["consult"],
            },
        )
        self.assertEqual(dict(self.audit.deferred), {"site-defer": 1})
        self.assertEqual(dict(self.audit.deferred_bytes), {"site-defer": lens["defer"]})
        self.assertEqual(
            dict(self.audit.deferred_pair), {("site-defer", "rust_is_subtype_coded"): 1}
        )
        self.assertEqual(
            dict(self.audit.seam_call_bytes), {"rust_is_subtype_coded": sum(lens.values())}
        )
        self.assertEqual(self.audit.pending, {})

    def test_audit_wrapper_splits_batch_slots_by_outcome(self) -> None:
        deferred_pair = [b"batch-left", b"batch-right"]
        decided_pair = [b"batch2-left", b"batch2-right"]
        for blob in deferred_pair + decided_pair:
            self.audit.register_serializer_result(blob, "site-batch")
        deferred_seam = self.audit.make_seam("rust_is_subtype_batch", lambda *a: [1, -1, 0])
        deferred_seam(deferred_pair)
        # One -1 slot defers the call: both blobs land in the deferred
        # bucket, the any(-1) rule the report's byte split depends on.
        self.assertEqual(dict(self.audit.seam_defers), {"rust_is_subtype_batch": 1})
        self.assertEqual(dict(self.audit.deferred), {"site-batch": 2})
        decided_seam = self.audit.make_seam("rust_is_subtype_batch", lambda *a: [1, 0])
        decided_seam(decided_pair)
        self.assertEqual(dict(self.audit.seam_defers), {"rust_is_subtype_batch": 1})
        self.assertEqual(dict(self.audit.useful), {"site-batch": 2})
        self.assertEqual(dict(self.audit.deferred), {"site-batch": 2})
        self.assertEqual(self.audit.pending, {})

    def test_share_proxy_counts_each_coded_decision(self) -> None:
        results = [1, 0, 3, -1, 1]
        proxy = self.share.CountingProxy("rust_is_subtype_coded", lambda *a: results.pop(0))
        # The proxy must pass the code through untouched: the shim
        # persists the exact answer it receives (#33).
        seen = [proxy() for _ in range(5)]
        self.assertEqual(seen, [1, 0, 3, -1, 1])
        self.assertEqual((proxy.calls, proxy.native, proxy.fallback), (5, 4, 1))

    def test_share_proxy_counts_batch_slots_per_decision(self) -> None:
        proxy = self.share.CountingProxy("rust_is_subtype_batch", lambda *a: [1, -1, 0, -1])
        self.assertEqual(proxy(), [1, -1, 0, -1])
        # One call, four decisions: two answered, two deferred.
        self.assertEqual((proxy.calls, proxy.native, proxy.fallback), (1, 2, 2))


class ShareReportDisplaySuite(unittest.TestCase):
    """#36: the share report's display must not round deferrals away."""

    def setUp(self) -> None:
        self.share = _load_share_module()

    def seeded(self, name: str, calls: int, native: int, fallback: int) -> Any:
        # The display reads the counters only; classification is proven by
        # CodedSeamSplitSuite, so the counters are seeded directly to keep
        # the #35 review shape cheap to reproduce.
        proxy = self.share.CountingProxy(name, lambda *a: None)
        proxy.calls, proxy.native, proxy.fallback = calls, native, fallback
        return proxy

    def sections(self, lines: list[str]) -> tuple[list[str], list[str]]:
        defer_at = lines.index("top deferrals by deferral count:")
        all_at = lines.index("all seams with calls:")
        return lines[defer_at + 1 : all_at - 1], lines[all_at + 1 :]

    def test_a_99_72_percent_native_seam_is_not_printed_as_100(self) -> None:
        # The #35 review shape: 22,932 calls, 64 deferrals. The :.0f share
        # printed "100% native" and hid them from the per-seam section.
        proxies = {
            "rust_is_subtype_coded": self.seeded("rust_is_subtype_coded", 22_932, 22_868, 64)
        }
        defer_rows, all_rows = self.sections(self.share.per_seam_report(proxies))
        self.assertIn("  rust_is_subtype_coded: 22932 calls (99.72% native)", all_rows)
        self.assertFalse(any("100% native" in row for row in all_rows))
        # The deferral stays visible with an unrounded share.
        self.assertIn(
            "  rust_is_subtype_coded: 22932 calls, 64 fallbacks (0.28% defer)", defer_rows
        )

    def test_the_deferral_list_ranks_by_deferrals_not_calls(self) -> None:
        # Sixteen bulk seams with more calls but fewer deferrals used to
        # fill the top-15-by-calls list and cut the coded seam entirely.
        proxies = {
            f"rust_bulk_{i:02d}": self.seeded(f"rust_bulk_{i:02d}", 200_000, 199_990, 10)
            for i in range(16)
        }
        proxies["rust_is_subtype_coded"] = self.seeded("rust_is_subtype_coded", 22_932, 22_868, 64)
        defer_rows, _ = self.sections(self.share.per_seam_report(proxies))
        # 17 seams defer, the list holds 15: call-count ranking cut the
        # coded seam (rank 17 by calls); deferral ranking leads with it.
        self.assertEqual(len(defer_rows), 15)
        self.assertEqual(
            defer_rows[0], "  rust_is_subtype_coded: 22932 calls, 64 fallbacks (0.28% defer)"
        )

    def test_main_prints_the_per_seam_report(self) -> None:
        # Wiring proof: `main()` must print what `per_seam_report` returns
        # for the counters the run produced, so the probe's exit report
        # cannot silently lose the deferral rows after the #36 refactor.
        proxies = {
            "rust_is_subtype_coded": self.seeded("rust_is_subtype_coded", 22_932, 22_868, 64)
        }
        err = io.StringIO()
        with (
            mock.patch.object(self.share, "run", lambda cwd: proxies),
            mock.patch.object(sys, "argv", ["measure_native_share.py"]),
            contextlib.redirect_stderr(err),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(self.share.main(), 0)
        output = err.getvalue()
        self.assertIn("top deferrals by deferral count:", output)
        self.assertIn("22932 calls, 64 fallbacks (0.28% defer)", output)
        self.assertIn("22932 calls (99.72% native)", output)
        self.assertNotIn("100% native", output)

    def test_batch_seam_share_divides_by_decisions_not_calls(self) -> None:
        # #41: a batch seam answers many slots per call, so the share
        # denominator is the decision total (native + fallback), never
        # the call count: one call, two slots, one deferred = 50.00%.
        proxies = {"rust_is_subtype_batch": self.seeded("rust_is_subtype_batch", 1, 1, 1)}
        defer_rows, all_rows = self.sections(self.share.per_seam_report(proxies))
        self.assertIn("  rust_is_subtype_batch: 1 calls, 1 fallbacks (50.00% defer)", defer_rows)
        self.assertIn("  rust_is_subtype_batch: 1 calls (50.00% native)", all_rows)

    def test_zero_decision_batch_seam_does_not_crash_the_display(self) -> None:
        # #46: a batch seam that answers zero pairs (empty list) has
        # calls > 0 but zero decisions, so dividing by the decision total
        # raised ZeroDivisionError and lost the whole report mid-print.
        proxies = {"rust_is_subtype_batch": self.seeded("rust_is_subtype_batch", 1, 0, 0)}
        defer_rows, all_rows = self.sections(self.share.per_seam_report(proxies))
        self.assertEqual(defer_rows, [])
        self.assertIn("  rust_is_subtype_batch: 1 calls (no decisions)", all_rows)

    def test_main_header_shares_divide_by_decisions_not_calls(self) -> None:
        # Same mismatch in main(): with calls (1) != decisions (2) the
        # per-call denominator printed 100% native and a zero fallback.
        proxies = {"rust_is_subtype_batch": self.seeded("rust_is_subtype_batch", 1, 1, 1)}
        err = io.StringIO()
        with (
            mock.patch.object(self.share, "run", lambda cwd: proxies),
            mock.patch.object(sys, "argv", ["measure_native_share.py"]),
            contextlib.redirect_stderr(err),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(self.share.main(), 0)
        output = err.getvalue()
        self.assertIn("total seam calls: 1", output)
        self.assertIn("total decisions:  2", output)
        self.assertIn("native:          1 (50.0%)", output)
        self.assertIn("python fallback: 1 (50.0%)", output)


if __name__ == "__main__":
    unittest.main()
