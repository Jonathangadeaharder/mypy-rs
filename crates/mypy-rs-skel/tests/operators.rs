//! Arithmetic-operator slice tests for issue #150 (brief §5; gates.rs
//! holds the manifest partition, these extend the battery).
//!
//! Differential: every operator arm of the manifest checks with output
//! byte-identical to the `.expected` fixture the generator captured
//! from real mypy, including the arms whose whole content is mypy
//! error output (the operator, left-operand and unary rows).
//!
//! Error order: augmented assignments desugar to a binop plus the
//! assignment check, so one statement can render both an operator and
//! an assignment error on one line. The bytes below were captured from
//! real mypy: the forward-variant error renders after the assignment
//! error (the right operand sorts after the statement start), while a
//! reflected-only variant ties the operator error at the statement
//! column and mypy prints it first, which the stable (line, col) sort
//! reproduces from the checker's push order.
//!
//! Subset rejects: `**` (typed Any by mypy, no Any-compatibility
//! semantics in the skeleton), augmented assignments whose target is
//! not an existing top-level local, and operator errors surfacing in
//! an imported sibling: those hard-reject (exit 2) naming the
//! sibling's path, never render as main-file diagnostics.

#![cfg(feature = "skel")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn assert_differential(file: &str, want_code: i32) {
    let dir = testdata_dir();
    let output = run_bin_in(&dir, file);
    assert_eq!(
        output.status.code(),
        Some(want_code),
        "exit code for {file}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stem = Path::new(file).file_stem().expect("file has no stem");
    let expected = fs::read(dir.join(format!("{}.expected", stem.to_string_lossy())))
        .expect("missing .expected fixture");
    assert_eq!(output.stdout, expected, "stdout bytes for {file}");
    assert!(
        output.stderr.is_empty(),
        "stderr for {file} must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The operator arms of the manifest (#150) are byte-identical to real
/// mypy, both the clean families and the error-row families.
#[test]
fn operator_arms_differential() {
    let cases = [
        ("expressions_arithmetic_int.py", 0),
        ("expressions_arithmetic_float.py", 1),
        ("expressions_arithmetic_str.py", 0),
        ("expressions_arithmetic_bool.py", 0),
        ("expressions_unary_operator.py", 0),
        ("expressions_operator_unsupported_operands.py", 1),
        ("expressions_operator_unsupported_left.py", 1),
        ("expressions_augmented_assignment.py", 1),
    ];
    for (file, code) in cases {
        assert_differential(file, code);
    }
}

/// `**` is excluded by decision 2 of the brief: mypy types it Any and
/// the skeleton has no Any-compatibility semantics, so it loud-rejects
/// at lowering time instead of risking the #93 silent class.
#[test]
fn pow_rejects_at_lowering() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-operators-pow");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::write(dir.join("pow.py"), "n = 2 ** 3\n").expect("write case");
    let output = run_bin_in(&dir, "pow.py");
    assert_eq!(output.status.code(), Some(2), "exit code");
    assert!(output.stdout.is_empty(), "no stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("binary operator `**` is outside the skeleton subset"),
        "stderr: {stderr}"
    );
}

/// One augmented assignment renders two errors on one line; mypy's
/// order is fixed by the error columns, which the bytes capture.
#[test]
fn augmented_assignment_error_order() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-operators-order");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "aug_fwd_order.py",
            concat!("v: bool = True\n", "v += \"s\"\n",),
            concat!(
                "aug_fwd_order.py:2: error: Incompatible types in assignment ",
                "(expression has type \"int\", variable has type \"bool\")  [assignment]\n",
                "aug_fwd_order.py:2: error: Unsupported operand types for + ",
                "(\"bool\" and \"str\")  [operator]\n",
                "Found 2 errors in 1 file (checked 1 source file)\n",
            ),
        ),
        (
            "aug_refl_order.py",
            concat!("s: str = \"a\"\n", "s -= 1\n",),
            concat!(
                "aug_refl_order.py:2: error: Unsupported operand types for - ",
                "(\"str\" and \"int\")  [operator]\n",
                "aug_refl_order.py:2: error: Incompatible types in assignment ",
                "(expression has type \"int\", variable has type \"str\")  [assignment]\n",
                "Found 2 errors in 1 file (checked 1 source file)\n",
            ),
        ),
    ];
    for (name, source, expect) in cases {
        fs::write(dir.join(name), source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(1),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expect,
            "stdout for {name}"
        );
        assert!(output.stderr.is_empty(), "stderr for {name}");
    }
}

/// An operator error inside an imported sibling hard-rejects naming the
/// sibling's path, never renders as a main-file diagnostic: the
/// renderer owns exactly one path, the main file's.
#[test]
fn sibling_operator_error_rejects_rather_than_renders() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-operators-sibling");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::write(
        dir.join("sib.py"),
        "def g() -> float:\n    return 1.5 + \"x\"\n",
    )
    .expect("write sibling");
    fs::write(dir.join("main.py"), "import sib\n").expect("write main");
    let output = run_bin_in(&dir, "main.py");
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "no stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sib.py:"),
        "the sibling's own path: {stderr}"
    );
    assert!(
        stderr.contains("operator type errors are outside the supported error classes"),
        "stderr: {stderr}"
    );
}

/// Augmented assignments the lowering cannot model are hard exit-2
/// rejections (#93: never a silent divergence): a target that is not a
/// plain name, a target that no earlier statement bound, and an
/// augmented assignment inside a conditional.
#[test]
fn subset_rejects_diverging_augmented_assignments() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-operators-reject");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "nonname_target.py",
            concat!(
                "class C:\n",
                "    pass\n",
                "\n",
                "\n",
                "c = C()\n",
                "c.x += 1\n",
            ),
            "augmented assignment target must be a single plain name",
        ),
        (
            "missing_local.py",
            "def f() -> int:\n    x += 1\n    return 1\n",
            "augmented assignment to a name that is not an existing local \
             variable is outside the skeleton subset",
        ),
        (
            "aug_in_conditional.py",
            "def f(x: int) -> int:\n    if x:\n        x += 1\n    return x\n",
            "augmented assignments inside conditionals are outside the skeleton subset",
        ),
    ];
    for (name, source, needle) in cases {
        fs::write(dir.join(name), source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(2),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "no stdout for {name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "stderr for {name}: {stderr}");
    }
}
