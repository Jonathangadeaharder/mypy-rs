"""Upstream corpus gate for the mypy-rs skeleton (issue #119).

Runs the standalone skeleton binary over curated cases from the upstream
mypy `.test` corpus and enforces the reader-fixed anti-#93 rule: a case is
allowed to fail only when it resolves to a manifest-declared unsupported
capability and the skeleton rejects it with exactly that capability's
marker. A case whose skeleton output differs from mypy on a declared
supported construct is always a failure; the allowlist can never mean
"skeleton output differs from mypy".

Self-contained on purpose: no mypy import, stdlib only, so the gate cannot
resolve a foreign tree through the editable-install finder (#1789).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_SEED = REPO_ROOT / "crates/mypy-rs-skel/testdata/upstream_seed.json"
DEFAULT_MANIFEST = REPO_ROOT / "crates/mypy-rs-skel/testdata/manifest.json"

PASS = "PASS"
UNSUPPORTED = "UNSUPPORTED"
SEMANTIC_DIFF = "SEMANTIC_DIFF"
UNCLASSIFIED = "UNCLASSIFIED"

SUCCESS_LINE = "Success: no issues found in 1 source file"


class GateError(Exception):
    """Usage or infrastructure failure: bad seed, missing binary, crash."""


@dataclass
class Case:
    """One `[case ...]` section: the implicit main program plus sections."""

    main: list[str] = field(default_factory=list)
    out: list[str] = field(default_factory=list)
    extras: list[str] = field(default_factory=list)


@dataclass
class ManifestCapability:
    id: str
    status: str
    reject_marker: str | None


@dataclass
class Classification:
    label: str
    detail: str = ""


def collapse_continuations(lines: list[str]) -> list[str]:
    r"""Mirror data.py: a trailing backslash joins the next line (leading
    spaces of the continuation are dropped)."""
    result: list[str] = []
    continuing = False
    for line in lines:
        joined = re.sub(r"\\$", "", line)
        if continuing:
            result[-1] += re.sub("^ +", "", joined)
        else:
            result.append(joined)
        continuing = line.endswith("\\")
    return result


def strip_list(lines: list[str]) -> list[str]:
    """Mirror data.py: drop trailing whitespace and trailing blank lines."""
    result = [re.sub(r"\s+$", "", line) for line in lines]
    while result and result[-1] == "":
        result.pop()
    return result


def parse_test_cases(text: str) -> dict[str, Case]:
    r"""Parse the subset of the upstream data format the gate needs.

    Mirrors mypy.test.data.parse_test_data for single-file cases: the
    `[case name]` header owns the implicit main program, `[out]` carries
    expected output, `\[` escapes a literal bracket, `--` starts a comment
    (`----` keeps `--` as content) and `\[`-free section headers end each
    block.
    """
    sections: list[tuple[str, str, list[str]]] = []
    current: tuple[str, str, list[str]] | None = None
    for line in text.split("\n"):
        stripped = line.strip()
        if line.startswith("[") and stripped.endswith("]"):
            if current is not None:
                sections.append(current)
            inner = stripped[1:-1]
            section_id, _, arg = inner.partition(" ")
            current = (section_id, arg, [])
        elif line.startswith("\\["):
            if current is not None:
                current[2].append(line[1:])
        elif line.startswith("--"):
            if line.startswith("----") and current is not None:
                current[2].append(line[2:])
        elif current is not None:
            current[2].append(line)
    if current is not None:
        sections.append(current)

    cases: dict[str, Case] = {}
    current: str | None = None
    for section_id, arg, raw in sections:
        data = strip_list(collapse_continuations(raw))
        if section_id == "case":
            cases[arg] = Case(main=data)
            current = arg
            continue
        if current is None:
            raise GateError(f"section [{section_id}] appears before any [case]")
        target = cases[current]
        if section_id in ("out", "out1") and not arg:
            target.out = data
        else:
            target.extras.append(section_id if not arg else f"{section_id} {arg}")
    return cases


def inline_errors(lines: list[str]) -> list[str]:
    """Mirror data.py expand_errors: `# E:`/`# N:` comments become
    `main:LINE: severity: message` output lines."""
    output: list[str] = []
    for i, line in enumerate(lines):
        for comment in line.split(" # ")[1:]:
            m = re.search(r"^([ENW]):((?P<col>\d+):)? (?P<message>.*)$", comment.strip())
            if m:
                severity = {"E": "error", "N": "note", "W": "warning"}[m.group(1)]
                message = m.group("message").replace("\\#", "#")
                if m.group("col") is None:
                    output.append(f"main:{i + 1}: {severity}: {message}")
                else:
                    output.append(f"main:{i + 1}:{m.group('col')}: {severity}: {message}")
    return output


def expected_result(case: Case) -> tuple[int, str]:
    """The mypy answer the skeleton must reproduce byte-for-byte.

    Expected errors are `[out]` lines plus inline `# E:` comments, with the
    corpus's `main:` module prefix renamed to the `main.py:` file prefix the
    skeleton renders. No tolerance is applied: a byte difference on a
    supported construct is the failure this gate exists to catch.
    """
    for line in case.main:
        if line.startswith("# flags:"):
            raise GateError(
                "case sets # flags, which the single-config skeleton driver cannot reproduce"
            )
    if case.extras:
        raise GateError(
            "case uses sections beyond [out] "
            f"({', '.join(case.extras)}); the gate only reproduces single-file cases"
        )
    errors = list(case.out) + inline_errors(case.main)
    for line in errors:
        if not line.startswith("main:"):
            raise GateError(f"expected output line {line!r} is not a main.py diagnostic")
        if re.match(r"^main:\d+:\d+", line):
            raise GateError(
                f"expected output line {line!r} carries a column number; "
                "the skeleton renders none, so byte comparison cannot hold"
            )
    if not errors:
        return 0, SUCCESS_LINE + "\n"
    lines = [line.replace("main:", "main.py:", 1) for line in errors]
    noun = "error" if len(lines) == 1 else "errors"
    lines.append(f"Found {len(lines)} {noun} in 1 file (checked 1 source file)")
    return 1, "\n".join(lines) + "\n"


def load_manifest(path: Path) -> dict[str, ManifestCapability]:
    raw = json.loads(path.read_text(encoding="utf-8"))
    caps = {}
    for entry in raw["capabilities"]:
        caps[entry["id"]] = ManifestCapability(
            id=entry["id"], status=entry["status"], reject_marker=entry.get("reject_marker")
        )
    return caps


def find_binary(explicit: str | None) -> Path:
    candidates: list[Path] = []
    if explicit:
        candidates.append(Path(explicit))
    elif os.environ.get("MYPY_RS_BIN"):
        candidates.append(Path(os.environ["MYPY_RS_BIN"]))
    else:
        candidates.append(REPO_ROOT / "target/debug/mypy-rs")
        candidates.append(REPO_ROOT / "target/release/mypy-rs")
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    raise GateError(
        "mypy-rs binary not found; build it with "
        "`cargo build -p mypy-rs-skel --features skel` or pass --bin / MYPY_RS_BIN"
    )


def run_skeleton(binary: Path, program: str) -> tuple[int, str, str]:
    with tempfile.TemporaryDirectory() as tmp:
        main = Path(tmp) / "main.py"
        main.write_text(program, encoding="utf-8")
        try:
            proc = subprocess.run(
                [str(binary), "main.py"], cwd=tmp, capture_output=True, text=True, timeout=60
            )
        except subprocess.TimeoutExpired as err:
            raise GateError(f"skeleton timed out on main.py: {err}") from err
    return proc.returncode, proc.stdout, proc.stderr


def classify(
    entry: dict[str, str], case: Case, capability: ManifestCapability, binary: Path
) -> Classification:
    """One case against the anti-#93 partition.

    Supported: byte-identical output or SEMANTIC_DIFF (hard failure). The
    allowlist may never mean "differs from mypy", so an unsupported-declared
    case that the skeleton answers instead of rejecting is also a
    SEMANTIC_DIFF. Exit 2 without the declared marker lands in the tracked
    UNCLASSIFIED bucket.
    """
    if capability.status == "unsupported" and not (capability.reject_marker or "").strip():
        raise GateError(f"manifest entry {capability.id} is unsupported without a marker")
    program = "\n".join(case.main) + "\n" if case.main else ""
    code, stdout, stderr = run_skeleton(binary, program)
    if capability.status == "supported":
        want_code, want_stdout = expected_result(case)
        if code == want_code and stdout == want_stdout and not stderr:
            return Classification(PASS)
        return Classification(
            SEMANTIC_DIFF,
            f"supported capability {capability.id} diverges from mypy: "
            f"exit {code} (want {want_code}), stdout {stdout.strip()!r}, "
            f"stderr {stderr.strip()!r}",
        )
    if code == 2:
        marker = capability.reject_marker
        assert marker is not None
        if marker in stderr:
            return Classification(UNSUPPORTED, marker)
        return Classification(
            UNCLASSIFIED, f"exit 2 without the {capability.id} marker: {stderr.strip()!r}"
        )
    return Classification(
        SEMANTIC_DIFF,
        f"manifest declares {capability.id} unsupported, but the skeleton answered "
        f"instead of rejecting: exit {code}, stdout {stdout.strip()!r}",
    )


def load_seed(path: Path) -> dict:
    seed = json.loads(path.read_text(encoding="utf-8"))
    for key in ("baseline", "cases"):
        if key not in seed:
            raise GateError(f"seed {path} lacks the {key!r} key")
    return seed


def resolve_case_file(entry: dict[str, str]) -> Path:
    file = Path(entry["file"])
    if not file.is_absolute():
        file = REPO_ROOT / file
    if not file.is_file():
        raise GateError(f"seed references missing corpus file {entry['file']}")
    return file


def run_gate(seed_path: Path, manifest_path: Path, binary: Path) -> tuple[dict, list[str]]:
    manifest = load_manifest(manifest_path)
    seed = load_seed(seed_path)
    parsed_files: dict[Path, dict[str, Case]] = {}
    results: list[tuple[dict[str, str], Classification]] = []
    for entry in seed["cases"]:
        if entry["capability"] not in manifest:
            raise GateError(f"seed case {entry['case']} names unknown capability")
        path = resolve_case_file(entry)
        if path not in parsed_files:
            parsed_files[path] = parse_test_cases(path.read_text(encoding="utf-8"))
        case = parsed_files[path].get(entry["case"])
        if case is None:
            raise GateError(f"seed references missing case {entry['case']} in {entry['file']}")
        results.append((entry, classify(entry, case, manifest[entry["capability"]], binary)))

    metrics = {PASS: 0, UNSUPPORTED: 0, SEMANTIC_DIFF: 0, UNCLASSIFIED: 0}
    lines: list[str] = []
    for entry, result in results:
        metrics[result.label] += 1
        where = f"{Path(entry['file']).name}::{entry['case']}"
        lines.append(f"{result.label:<12} {where} ({entry['capability']})")
        if result.detail:
            lines.append(f"             {result.detail}")
    return metrics, lines


def check_baseline(seed: dict, metrics: dict[str, int]) -> tuple[list[str], list[str]]:
    """The ratchet: semantic diffs are always fatal, the allowlist buckets
    may never grow. Shrinking is green and prints a ratchet hint."""
    baseline = seed["baseline"]
    problems: list[str] = []
    hints: list[str] = []
    if baseline.get("semantic_diff", -1) != 0 or metrics[SEMANTIC_DIFF] != 0:
        problems.append(
            f"semantic-diff count is {metrics[SEMANTIC_DIFF]} (baseline "
            f"{baseline.get('semantic_diff')}); the allowlist may never mean "
            '"skeleton output differs from mypy"'
        )
    for key, label in (("unsupported", UNSUPPORTED), ("unclassified", UNCLASSIFIED)):
        observed = metrics[label]
        allowed = baseline.get(key, 0)
        if observed > allowed:
            problems.append(
                f"{key} count grew: {observed} observed, baseline {allowed}; "
                "curate the seed and update the baseline in the same commit"
            )
        elif observed < allowed:
            hints.append(f"ratchet: {key} shrank {allowed} -> {observed}; run --update-baseline")
    return problems, hints


def update_baseline(seed_path: Path, seed: dict, metrics: dict[str, int]) -> None:
    if metrics[SEMANTIC_DIFF] != 0:
        raise GateError("refusing to ratchet a semantic difference into the baseline")
    seed["baseline"] = {
        "pass": metrics[PASS],
        "unsupported": metrics[UNSUPPORTED],
        "semantic_diff": 0,
        "unclassified": metrics[UNCLASSIFIED],
    }
    seed_path.write_text(json.dumps(seed, indent=2) + "\n", encoding="utf-8")


SYNTHETIC_CORPUS = """\
[case testSynthClean]
x: int = 1

[case testSynthWrongExpected]
x: int = 1
[out]
main:1: error: no such error

[case testSynthClassSupported]
class C:
    pass

[case testSynthAsyncReject]
async def f() -> int:
    return 1

[case testSynthIfReject]
if True:
    pass
"""


def synthetic_seed(corpus: Path, case: str, capability: str, baseline: dict[str, int]) -> dict:
    return {
        "baseline": baseline,
        "cases": [{"file": str(corpus), "case": case, "capability": capability}],
    }


def run_self_test(binary: Path, manifest_path: Path) -> int:
    """Falsification for the classification contract itself.

    Proves on a synthetic corpus that (a) a supported case whose expected
    bytes the skeleton cannot reproduce is reported SEMANTIC_DIFF and fails
    the run, and (b) a reject whose marker matches no manifest capability is
    reported UNCLASSIFIED and fails the run. A clean case and a proper
    unsupported reject must both stay green.
    """
    with tempfile.TemporaryDirectory() as tmp:
        corpus = Path(tmp) / "check-synth.test"
        corpus.write_text(SYNTHETIC_CORPUS, encoding="utf-8")
        zero = {"pass": 0, "unsupported": 0, "semantic_diff": 0, "unclassified": 0}
        one_unsupported = {**zero, "unsupported": 1}
        scenarios = [
            (
                "control-pass",
                synthetic_seed(corpus, "testSynthClean", "assignments.annotated_primitives", zero),
                0,
                [PASS],
            ),
            (
                "control-pass-class",
                synthetic_seed(corpus, "testSynthClassSupported", "classes.basic", zero),
                0,
                [PASS],
            ),
            (
                "control-unsupported",
                synthetic_seed(corpus, "testSynthAsyncReject", "functions.async", one_unsupported),
                0,
                [UNSUPPORTED],
            ),
            (
                "semantic-diff",
                synthetic_seed(
                    corpus, "testSynthWrongExpected", "assignments.annotated_primitives", zero
                ),
                1,
                [SEMANTIC_DIFF],
            ),
            (
                "unclassified",
                synthetic_seed(corpus, "testSynthIfReject", "functions.async", zero),
                1,
                [UNCLASSIFIED],
            ),
        ]
        failures = 0
        for name, seed, want_code, needles in scenarios:
            seed_path = Path(tmp) / f"seed-{name}.json"
            seed_path.write_text(json.dumps(seed, indent=2) + "\n", encoding="utf-8")
            proc = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).resolve()),
                    "--bin",
                    str(binary),
                    "--manifest",
                    str(manifest_path),
                    "--seed",
                    str(seed_path),
                ],
                capture_output=True,
                text=True,
                timeout=120,
            )
            ok = proc.returncode == want_code and all(needle in proc.stdout for needle in needles)
            print(f"self-test {name}: {'ok' if ok else 'FAILED'} (exit {proc.returncode})")
            if not ok:
                failures += 1
                print(proc.stdout)
                print(proc.stderr)
        print(f"self-test: {len(scenarios) - failures}/{len(scenarios)} scenarios ok")
        return 1 if failures else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--seed", type=Path, default=DEFAULT_SEED, help="seed JSON path")
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST, help="manifest path")
    parser.add_argument("--bin", help="mypy-rs binary path (default: MYPY_RS_BIN or target/)")
    parser.add_argument("--update-baseline", action="store_true", help="rewrite the seed baseline")
    parser.add_argument("--self-test", action="store_true", help="run the falsification scenarios")
    args = parser.parse_args(argv)

    try:
        binary = find_binary(args.bin)
        if args.self_test:
            return run_self_test(binary, args.manifest)
        metrics, lines = run_gate(args.seed, args.manifest, binary)
        for line in lines:
            print(line)
        print(
            f"skeleton upstream gate: {sum(metrics.values())} cases: "
            f"{metrics[PASS]} pass, {metrics[UNSUPPORTED]} unsupported, "
            f"{metrics[SEMANTIC_DIFF]} semantic-diff, {metrics[UNCLASSIFIED]} unclassified"
        )
        seed = load_seed(args.seed)
        problems, hints = check_baseline(seed, metrics)
        for hint in hints:
            print(f"note: {hint}")
        baseline = seed["baseline"]
        print(
            f"baseline: pass={baseline.get('pass')} unsupported={baseline.get('unsupported')} "
            f"semantic-diff={baseline.get('semantic_diff')} "
            f"unclassified={baseline.get('unclassified')}"
        )
        if args.update_baseline:
            if problems:
                for problem in problems:
                    print(f"FAIL: {problem}")
                print("refusing to ratchet: curate the seed and re-run")
                return 1
            update_baseline(args.seed, seed, metrics)
            print(f"baseline updated in {args.seed}")
            return 0
        for problem in problems:
            print(f"FAIL: {problem}")
        return 1 if problems else 0
    except GateError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
