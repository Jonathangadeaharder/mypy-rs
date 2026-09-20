//! Gate tests for issue #91 (brief §5). These run only with
//! `--features skel`; the feature never builds in production jobs.
//!
//! Gate 1: differential: bin output is byte-identical to the
//! expected files the generator captured from real mypy.
//! Gate 2: no-libpython: the bin links no Python and carries no
//! undefined Py symbols (the C API stubs are inert linker
//! artifacts, so `nm -u` must stay empty of them).
//! Gate 3: the registered production seam count in type_kernel is
//! unchanged by this lane.
//! Gate 4: cache isolation: a run leaves no cache artifacts and
//! does not mutate the corpus.
//! Manifest: gate 5 reads testdata/manifest.json (#115) and enforces
//! the three-way partition per capability: supported arms byte-identical
//! to Python mypy, unsupported arms hard-reject with the declared marker,
//! and a zero semantic-difference count across all arms.
//!
//! Gate 2 is macOS-only (otool/nm); on other targets it compiles away.
//! The whole file compiles to nothing without the `skel` feature: the
//! bin's required-features then skip the binary and `CARGO_BIN_EXE_mypy-rs`
//! is undefined, which `env!` would turn into a hard compile error.

#![cfg(feature = "skel")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

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

fn expected_path(file: &str) -> PathBuf {
    let stem = Path::new(file).file_stem().expect("file has no stem");
    let name = format!("{}.expected", stem.to_string_lossy());
    testdata_dir().join(name)
}

fn expected(file: &str) -> Vec<u8> {
    fs::read(expected_path(file)).expect("missing .expected fixture")
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

/// Constructs whose skeleton lowering would silently diverge from
/// mypy semantics must be hard subset errors (exit 2), never a
/// success with dropped information: `async def` (mypy checks the
/// return as a Coroutine, not the annotation), decorators (dropped
/// entirely would change the checked type) and PEP 695 type
/// parameters (generic semantics the skeleton does not model).
#[test]
fn subset_rejects_semantics_diverging_functions() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-subset-reject");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "async_def.py",
            "async def f() -> int:\n    return 1\n",
            "async functions are outside the skeleton subset",
        ),
        (
            "decorated_def.py",
            "@dec\ndef f() -> int:\n    return 1\n",
            "decorated functions are outside the skeleton subset",
        ),
        (
            "pep695_type_params.py",
            "def f[T]() -> int:\n    return 1\n",
            "PEP 695 type parameters are outside the skeleton subset",
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

/// A non-None return annotation with no returned value is a mypy error
/// ("Missing return statement"). The skeleton must reject it as
/// out-of-subset (exit 2), never report Success.
#[test]
fn subset_rejects_valueless_function_body() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-subset-reject");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "pass_body.py",
            "def f() -> int:\n    pass\n",
            "pass body is outside",
        ),
        (
            "bare_return.py",
            "def f() -> int:\n    return\n",
            "bare return is outside",
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

/// One capability in the committed support manifest (#115): the corpus
/// arm that exercises it, the partition verdict, and for unsupported
/// entries the exact reject marker the skeleton must emit.
#[derive(Deserialize)]
struct ManifestCapability {
    id: String,
    status: String,
    arm: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    reject_marker: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    capabilities: Vec<ManifestCapability>,
}

fn load_manifest() -> Manifest {
    let path = testdata_dir().join("manifest.json");
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

/// The three-way partition from #93 and #115. A supported arm must be
/// byte-identical to Python mypy; an unsupported arm must hard reject
/// with its declared marker. The two categories may never collapse:
/// a differing supported arm is a wrong answer, and an unsupported arm
/// with a recorded .expected file would be a semantic-difference
/// allowlist, which is forbidden outright.
#[test]
fn manifest_three_way_partition() {
    let manifest = load_manifest();
    assert!(
        !manifest.capabilities.is_empty(),
        "manifest has no capabilities"
    );
    let dir = testdata_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut supported = 0;
    let mut unsupported = 0;
    let mut semantic_differences = 0;
    for cap in &manifest.capabilities {
        let arm = cap.arm.as_str();
        if !dir.join(arm).is_file() {
            failures.push(format!("{}: arm {arm} is missing", cap.id));
            continue;
        }
        match cap.status.as_str() {
            "supported" => {
                supported += 1;
                if let Some(marker) = &cap.reject_marker {
                    failures.push(format!(
                        "{}: supported entry declares reject marker {marker:?}",
                        cap.id
                    ));
                    continue;
                }
                let Some(&want_code) = cap.exit_code.as_ref() else {
                    failures.push(format!("{}: supported entry has no exit_code", cap.id));
                    continue;
                };
                let want_bytes = match fs::read(expected_path(arm)) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        failures.push(format!(
                            "{}: cannot read .expected for arm {arm}: {e}",
                            cap.id
                        ));
                        continue;
                    }
                };
                let (code, stdout, stderr) = run_bin(arm);
                if !stderr.is_empty() {
                    failures.push(format!(
                        "{}: arm {arm} wrote stderr: {}",
                        cap.id,
                        String::from_utf8_lossy(&stderr)
                    ));
                }
                if stdout != want_bytes {
                    semantic_differences += 1;
                    failures.push(format!(
                        "{}: WRONG ANSWER, arm {arm} differs from Python mypy\n\
                         expected:\n{}\nobserved:\n{}",
                        cap.id,
                        String::from_utf8_lossy(&want_bytes),
                        String::from_utf8_lossy(&stdout),
                    ));
                }
                if code != want_code {
                    failures.push(format!(
                        "{}: arm {arm} exited {code}, manifest declares {want_code}",
                        cap.id
                    ));
                }
            }
            "unsupported" => {
                unsupported += 1;
                let Some(marker) = cap.reject_marker.as_deref() else {
                    failures.push(format!(
                        "{}: unsupported entry declares no reject_marker",
                        cap.id
                    ));
                    continue;
                };
                if marker.trim().is_empty() {
                    failures.push(format!(
                        "{}: unsupported entry declares an empty reject_marker; an empty \
                         substring matches any stderr",
                        cap.id
                    ));
                    continue;
                }
                let Some(&want_code) = cap.exit_code.as_ref() else {
                    failures.push(format!("{}: unsupported entry has no exit_code", cap.id));
                    continue;
                };
                if expected_path(arm).exists() {
                    failures.push(format!(
                        "{}: unsupported arm {arm} has an .expected file; unsupported and \
                         wrong-answer have collapsed",
                        cap.id
                    ));
                    continue;
                }
                let (code, stdout, stderr) = run_bin(arm);
                let stderr = String::from_utf8_lossy(&stderr);
                if code != want_code {
                    failures.push(format!(
                        "{}: arm {arm} exited {code}, manifest declares {want_code}; \
                         stderr: {stderr}",
                        cap.id
                    ));
                }
                if !stdout.is_empty() {
                    failures.push(format!(
                        "{}: arm {arm} wrote stdout: {}",
                        cap.id,
                        String::from_utf8_lossy(&stdout)
                    ));
                }
                if !stderr.contains(marker) {
                    failures.push(format!(
                        "{}: arm {arm} stderr lacks marker {marker:?}: {stderr}",
                        cap.id
                    ));
                }
            }
            other => failures.push(format!("{}: unknown status {other:?}", cap.id)),
        }
    }
    println!(
        "manifest: {} capabilities, {supported} supported, {unsupported} unsupported, \
         semantic differences: {semantic_differences}",
        manifest.capabilities.len()
    );
    assert!(
        supported > 0,
        "the supported category collapsed; the manifest must keep supported arms"
    );
    assert!(
        unsupported > 0,
        "the unsupported category collapsed; the manifest must keep unsupported arms"
    );
    assert_eq!(
        semantic_differences, 0,
        "no arm may differ from Python mypy; the semantic-difference count must stay zero"
    );
    assert!(
        failures.is_empty(),
        "manifest partition failures:\n{}",
        failures.join("\n")
    );
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

/// Count lines that *are* the attribute (any form, including
/// `#[pyclass(name = "...")]`), so comment mentions of `#[pyclass]` and
/// doc lines do not inflate the count. `attr` is the line-start prefix.
fn count_attr(root: &Path, attr: &str) -> usize {
    fn walk(dir: &Path, attr: &str, total: &mut usize) {
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}")) {
            let entry = entry.unwrap_or_else(|e| panic!("dir entry in {dir:?}: {e}"));
            let path = entry.path();
            if path.is_dir() {
                walk(&path, attr, total);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let content =
                    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
                *total += content
                    .lines()
                    .filter(|line| line.trim_start().starts_with(attr))
                    .count();
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
        count_attr(&kernel_src, "#[pyfunction"),
        848,
        "registered #[pyfunction] seams changed; this lane must not add or remove any"
    );
    assert_eq!(
        count_attr(&kernel_src, "#[pyclass"),
        7,
        "registered #[pyclass] seams changed; this lane must not add or remove any"
    );
}

#[test]
fn gate4_cache_and_corpus_isolation() {
    let dir = testdata_dir();
    // Control arms plus the manifest arms, deduplicated: empty_control.py
    // is both a gate 1 control and the module.pass_statement arm.
    let manifest = load_manifest();
    let mut arms = vec![TRIVIAL.to_string(), EMPTY_CONTROL.to_string()];
    for cap in &manifest.capabilities {
        if !arms.contains(&cap.arm) {
            arms.push(cap.arm.clone());
        }
    }
    let before: Vec<(PathBuf, Vec<u8>)> = arms
        .iter()
        .map(|f| {
            (
                dir.join(f),
                fs::read(dir.join(f)).expect("corpus read failed"),
            )
        })
        .collect();

    for arm in &arms {
        run_bin(arm);
    }

    assert!(
        !dir.join(".mypy_cache").exists(),
        "a run created .mypy_cache in the corpus directory"
    );
    for (path, bytes) in before {
        let after = fs::read(&path).expect("corpus re-read failed");
        assert_eq!(bytes, after, "{path:?} mutated by a run");
    }
}
