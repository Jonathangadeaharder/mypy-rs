//! Gate tests for issue #91 (brief §5). These run only with
//! `--features skel`; the feature never builds in production jobs.
//!
//! Gate 1: differential — bin output is byte-identical to the
//! expected files the generator captured from real mypy.
//! Gate 2: no-libpython — the bin links no Python and carries no
//! undefined Py symbols (the C API stubs are inert linker
//! artifacts, so `nm -u` must stay empty of them).
//! Gate 3: the registered production seam count in type_kernel is
//! unchanged by this lane.
//! Gate 4: cache isolation — a run leaves no cache artifacts and
//! does not mutate the corpus.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TRIVIAL: &str = "trivial.py";
const EMPTY_CONTROL: &str = "empty_control.py";

fn testdata_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

fn run_bin(file: &str) -> (i32, Vec<u8>, Vec<u8>) {
    let output = Command::new(env!("CARGO_BIN_EXE_mypy-rs"))
        .arg(file)
        .current_dir(testdata_dir())
        .output()
        .expect("failed to spawn mypy-rs");
    (
        output.status.code().expect("terminated by signal"),
        output.stdout,
        output.stderr,
    )
}

fn expected(file: &str) -> Vec<u8> {
    let stem = Path::new(file).file_stem().expect("file has no stem");
    let name = format!("{}.expected", stem.to_string_lossy());
    fs::read(testdata_dir().join(name)).expect("missing .expected fixture")
}

fn assert_differential(file: &str, want_code: i32) {
    let (code, stdout, stderr) = run_bin(file);
    assert_eq!(code, want_code, "exit code for {file}");
    assert_eq!(stdout, expected(file), "stdout bytes for {file}");
    assert!(
        stderr.is_empty(),
        "stderr for {file} must be empty: {}",
        String::from_utf8_lossy(&stderr)
    );
}

#[test]
fn gate1_differential_trivial_error() {
    assert_differential(TRIVIAL, 1);
}

#[test]
fn gate1_differential_empty_control_success() {
    assert_differential(EMPTY_CONTROL, 0);
}

#[cfg(target_os = "macos")]
#[test]
fn gate2_no_libpython() {
    let exe = env!("CARGO_BIN_EXE_mypy-rs");

    let otool = Command::new("otool")
        .arg("-L")
        .arg(exe)
        .output()
        .expect("otool failed");
    assert!(otool.status.success());
    let linked = String::from_utf8_lossy(&otool.stdout).to_lowercase();
    assert!(
        !linked.contains("python"),
        "mypy-rs must not link libpython, otool -L says: {linked}"
    );

    let nm = Command::new("nm")
        .arg("-u")
        .arg(exe)
        .output()
        .expect("nm failed");
    assert!(nm.status.success());
    let undefined = String::from_utf8_lossy(&nm.stdout);
    assert!(
        !undefined.contains("Py"),
        "no undefined C Python symbols allowed, nm -u says: {undefined}"
    );
}

fn count_attr(root: &Path, attr: &str) -> usize {
    fn walk(dir: &Path, attr: &str, total: &mut usize) {
        for entry in fs::read_dir(dir).expect("read_dir failed") {
            let entry = entry.expect("dir entry failed");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, attr, total);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let content = fs::read_to_string(&path).expect("read failed");
                *total += content.matches(attr).count();
            }
        }
    }
    let mut total = 0;
    walk(root, attr, &mut total);
    total
}

#[test]
fn gate3_production_seam_count_unchanged() {
    let kernel_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../type_kernel/src");
    assert_eq!(
        count_attr(&kernel_src, "#[pyfunction]"),
        992,
        "registered #[pyfunction] seams changed; this lane must not add or remove any"
    );
    assert_eq!(
        count_attr(&kernel_src, "#[pyclass]"),
        11,
        "registered #[pyclass] seams changed; this lane must not add or remove any"
    );
}

#[test]
fn gate4_cache_and_corpus_isolation() {
    let dir = testdata_dir();
    let before: Vec<(PathBuf, Vec<u8>)> = [TRIVIAL, EMPTY_CONTROL]
        .iter()
        .map(|f| {
            (
                dir.join(f),
                fs::read(dir.join(f)).expect("corpus read failed"),
            )
        })
        .collect();

    run_bin(TRIVIAL);
    run_bin(EMPTY_CONTROL);

    assert!(
        !dir.join(".mypy_cache").exists(),
        "a run created .mypy_cache in the corpus directory"
    );
    for (path, bytes) in before {
        let after = fs::read(&path).expect("corpus re-read failed");
        assert_eq!(bytes, after, "{path:?} mutated by a run");
    }
}
