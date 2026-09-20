//! Conditional control-flow slice tests for issue #136 (gates.rs is
//! frozen; these extend the battery without touching it).
//!
//! Differential: `control_flow.py` (if/elif/else bodies, comparisons,
//! boolean operators, `%`, local assignments, returns) checks clean with
//! output byte-identical to the `control_flow.expected` fixture the
//! generator captured from real mypy.
//!
//! Falsification: three one-token mutations (wrong return value, a
//! dropped return, a non-bool condition) must produce the exact bytes
//! real mypy emits for the same mutant. Notably the non-bool condition
//! stays clean in both checkers: truthy-bool is not a default-enabled
//! error code, so mypy accepts `if "x":`. The skeleton follows mypy,
//! not the issue's assumed error.

#![cfg(feature = "skel")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONTROL_FLOW: &str = "control_flow.py";

fn testdata_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

fn run_bin_in(dir: &Path, file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mypy-rs"))
        .arg(file)
        .current_dir(dir)
        .output()
        .expect("failed to spawn mypy-rs")
}

#[test]
fn control_flow_differential_success() {
    let output = run_bin_in(&testdata_dir(), CONTROL_FLOW);
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code for {CONTROL_FLOW}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected =
        fs::read(testdata_dir().join("control_flow.expected")).expect("missing .expected fixture");
    assert_eq!(output.stdout, expected, "stdout bytes for {CONTROL_FLOW}");
    assert!(
        output.stderr.is_empty(),
        "stderr for {CONTROL_FLOW} must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The differential run must not mutate the corpus and must leave no
/// cache directory behind.
#[test]
fn control_flow_corpus_isolation() {
    let dir = testdata_dir();
    let before: Vec<u8> = fs::read(dir.join(CONTROL_FLOW)).expect("corpus read failed");

    let output = run_bin_in(&dir, CONTROL_FLOW);
    assert_eq!(
        output.status.code(),
        Some(0),
        "isolation run of {CONTROL_FLOW} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !dir.join(".mypy_cache").exists(),
        "a run created .mypy_cache in the corpus directory"
    );
    let after = fs::read(dir.join(CONTROL_FLOW)).expect("corpus re-read failed");
    assert_eq!(before, after, "{CONTROL_FLOW} mutated by a run");
}

/// Real mypy on each mutant emits exactly these bytes; the skeleton
/// must match them, proving the slice checks the same corpus lines
/// with the same message rendering, not just exits 0 on the clean
/// corpus.
#[test]
fn control_flow_falsification_matches_mypy() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-control-flow-mutant");
    fs::create_dir_all(&dir).expect("temp dir");
    let source =
        fs::read_to_string(testdata_dir().join(CONTROL_FLOW)).expect("read control_flow.py");

    let mutants: [(&str, String, i32, &str); 3] = [
        (
            "wrong return value",
            source.replace("    return low", "    return \"low\""),
            1,
            concat!(
                "control_flow.py:3: error: Incompatible return value type ",
                "(got \"str\", expected \"float\")  [return-value]\n",
                "Found 1 error in 1 file (checked 1 source file)\n",
            ),
        ),
        (
            "missing return",
            source.replace("    return value", "    x: int = 1"),
            1,
            concat!(
                "control_flow.py:1: error: Missing return statement  [return]\n",
                "Found 1 error in 1 file (checked 1 source file)\n",
            ),
        ),
        (
            "non-bool condition",
            source.replace("    if value < low:", "    if \"x\":"),
            0,
            "Success: no issues found in 1 source file\n",
        ),
    ];
    for (label, mutant, code, expect) in mutants {
        assert_ne!(
            mutant, source,
            "the mutated line for {label} is no longer in the corpus; update this test"
        );
        fs::write(dir.join(CONTROL_FLOW), mutant).expect("write mutant");
        let output = run_bin_in(&dir, CONTROL_FLOW);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for the {label} mutant: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expect.as_bytes(), "stdout for {label}");
        assert!(
            output.stderr.is_empty(),
            "stderr for {label} must be empty: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Control-flow constructs whose lowering would silently diverge from
/// mypy semantics must be hard exit-2 rejections (#93: never a silent
/// divergence): loops, match, module-level if, chained comparisons,
/// membership tests, and/or over non-bool operands, unary `~`,
/// branch-local assignments, local rebinding and a
/// return before the final statement of its sequence.
#[test]
fn subset_rejects_control_flow_constructs() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-control-flow-reject");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "while_body.py",
            "def f() -> int:\n    while True:\n        pass\n    return 1\n",
            "statement `While` is outside the skeleton subset in a body",
        ),
        (
            "for_body.py",
            "def f(xs: int) -> int:\n    for x in xs:\n        pass\n    return 1\n",
            "statement `For` is outside the skeleton subset in a body",
        ),
        (
            "match_body.py",
            "def f(x: int) -> int:\n    match x:\n        case 1:\n            return 1\n    return 0\n",
            "statement `Match` is outside the skeleton subset in a body",
        ),
        (
            "module_if.py",
            "if 1 < 2:\n    x: int = 1\n",
            "statement `If` is outside the skeleton subset",
        ),
        (
            "chained_comparison.py",
            "x: bool = 1 < 2 < 3\n",
            "chained comparisons are outside the skeleton subset",
        ),
        (
            "membership.py",
            "x: bool = 1 in (1, 2)\n",
            "comparison operator `in` is outside the skeleton subset",
        ),
        (
            "identity.py",
            "x: bool = 1 is 1\n",
            "comparison operator `is` is outside the skeleton subset",
        ),
        (
            "non_bool_and.py",
            "x: int = 1 and 2\n",
            "boolean `and` is only supported over builtins.bool operands",
        ),
        (
            "unary_invert.py",
            "x: int = ~1\n",
            "unary operator `~` is outside the skeleton subset",
        ),
        (
            "branch_local.py",
            "def f(x: int) -> int:\n    if x:\n        y: int = 1\n    return x\n",
            "local assignments inside conditionals are outside the skeleton subset",
        ),
        (
            "local_rebind.py",
            "def f(x: int) -> int:\n    x: int = 1\n    return x\n",
            "rebinding a local variable is outside the skeleton subset",
        ),
        (
            "return_not_final.py",
            "def f(x: int) -> int:\n    return 1\n    pass\n",
            "a return before the final statement is outside the skeleton subset",
        ),
        (
            "self_rebind.py",
            "class C:\n    def m(self) -> None:\n        self = 5\n",
            "rebinding a local variable is outside the skeleton subset",
        ),
    ];
    for (name, source, needle) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = Command::new(env!("CARGO_BIN_EXE_mypy-rs"))
            .arg(&path)
            .output()
            .expect("spawn mypy-rs");
        assert_eq!(output.status.code(), Some(2), "exit code for {name}");
        assert!(output.stdout.is_empty(), "no stdout for {name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "stderr for {name}: {stderr}");
    }
}

/// A return-path incompatibility inside an imported sibling must
/// reject with the sibling's path (exit 2), never render as a
/// main-file diagnostic: the renderer owns exactly one path, the main
/// file's.
#[test]
fn sibling_return_errors_reject_rather_than_render() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-control-flow-sibling");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "sib.py",
            "def g() -> float:\n    return \"x\"\n",
            "return-value incompatibility is outside the supported error classes",
        ),
        (
            "sib.py",
            "def g() -> int:\n    y: int = 1\n",
            "a missing return statement is outside the supported error classes",
        ),
    ];
    for (label, source, needle) in cases {
        fs::write(dir.join("sib.py"), source).expect("write sibling");
        fs::write(dir.join("main.py"), "import sib\n").expect("write main");
        let output = run_bin_in(&dir, "main.py");
        assert_eq!(
            output.status.code(),
            Some(2),
            "exit code for {label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.is_empty(),
            "no stdout for a sibling incompatibility ({label})"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("sib.py:"),
            "the sibling's own path ({label}): {stderr}"
        );
        assert!(stderr.contains(needle), "stderr for {label}: {stderr}");
    }
}

/// A body that both renders a value-return error on a later line and
/// falls off the end must print mypy's line-sorted order (the def-line
/// missing-return first), not the checker's push order. Real mypy on
/// this input emits exactly these bytes.
#[test]
fn diagnostic_line_order_matches_mypy() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-control-flow-order");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::write(
        dir.join(CONTROL_FLOW),
        "def f(x: int) -> int:\n    if x > 0:\n        return \"s\"\n",
    )
    .expect("write order case");
    let output = run_bin_in(&dir, CONTROL_FLOW);
    assert_eq!(output.status.code(), Some(1), "exit code");
    assert_eq!(
        output.stdout,
        concat!(
            "control_flow.py:1: error: Missing return statement  [return]\n",
            "control_flow.py:3: error: Incompatible return value type ",
            "(got \"str\", expected \"int\")  [return-value]\n",
            "Found 2 errors in 1 file (checked 1 source file)\n",
        )
        .as_bytes(),
        "line order must match mypy"
    );
    assert!(output.stderr.is_empty(), "stderr must be empty");
}
