//! Class/member slice tests for issue #118 (brief §5; gates.rs took
//! the sanctioned #121 hardening pass, these extend the battery).
//!
//! Differential: the grown corpus (`class_slice.py` +
//! `class_slice_base.py`, generic inheritance, methods, super() calls,
//! cross-module classes) checks clean with output byte-identical to
//! the `class_slice.expected` fixture the generator captured from real
//! mypy, and a run leaves the corpus files and the directory untouched
//! (gates.rs gate4 covers only the trivial corpus).
//!
//! Falsification: a one-token mutation (line 39 `float` -> `str`) must
//! produce the exact bytes real mypy emits for the same mutant, exit 1.
//! The expected bytes below were captured from real mypy on this tree
//! and verified byte-identical to the skeleton before hardcoding.
//!
//! Subset rejects: class-level constructs the slice does not model are
//! hard exit-2 rejections (#93: never a silent divergence).

#![cfg(feature = "skel")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CLASS_SLICE: &str = "class_slice.py";
const CLASS_SLICE_BASE: &str = "class_slice_base.py";

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
fn class_slice_differential_success() {
    let output = run_bin_in(&testdata_dir(), CLASS_SLICE);
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code for {CLASS_SLICE}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected =
        fs::read(testdata_dir().join("class_slice.expected")).expect("missing .expected fixture");
    assert_eq!(output.stdout, expected, "stdout bytes for {CLASS_SLICE}");
    assert!(
        output.stderr.is_empty(),
        "stderr for {CLASS_SLICE} must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The differential run must not mutate the class corpus (gates.rs
/// gate4 covers only the trivial corpus) and must leave no cache dir.
#[test]
fn class_slice_corpus_isolation() {
    let dir = testdata_dir();
    let before: Vec<(PathBuf, Vec<u8>)> = [CLASS_SLICE, CLASS_SLICE_BASE]
        .iter()
        .map(|f| {
            (
                dir.join(f),
                fs::read(dir.join(f)).expect("corpus read failed"),
            )
        })
        .collect();

    let output = run_bin_in(&dir, CLASS_SLICE);
    assert_eq!(
        output.status.code(),
        Some(0),
        "isolation run of {CLASS_SLICE} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !dir.join(".mypy_cache").exists(),
        "a run created .mypy_cache in the corpus directory"
    );
    for (path, bytes) in before {
        let after = fs::read(&path).expect("corpus re-read failed");
        assert_eq!(bytes, after, "{path:?} mutated by a run");
    }
}

/// Real mypy on the mutant emits exactly these bytes (exit 1); the
/// skeleton must match them, proving the class/member slice checks the
/// same corpus lines with the same message rendering, not just exits 0
/// on the clean corpus.
#[test]
fn class_slice_falsification_matches_mypy() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-mutant");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::copy(
        testdata_dir().join(CLASS_SLICE_BASE),
        dir.join(CLASS_SLICE_BASE),
    )
    .expect("copy class_slice_base.py");
    let source = fs::read_to_string(testdata_dir().join(CLASS_SLICE)).expect("read class_slice.py");
    let mutant = source.replace(
        "area: float = combined_area(circle, square)",
        "area: str = combined_area(circle, square)",
    );
    assert_ne!(
        mutant, source,
        "the mutated line is no longer in the corpus; update this test"
    );
    fs::write(dir.join(CLASS_SLICE), mutant).expect("write mutant");

    let output = run_bin_in(&dir, CLASS_SLICE);
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code for the mutant: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = concat!(
        "class_slice.py:39: error: Incompatible types in assignment ",
        "(expression has type \"float\", variable has type \"str\")  [assignment]\n",
        "Found 1 error in 1 file (checked 1 source file)\n",
    );
    assert_eq!(output.stdout, expected.as_bytes(), "stdout for the mutant");
    assert!(
        output.stderr.is_empty(),
        "stderr for the mutant must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Class-level constructs whose lowering would silently diverge from
/// mypy semantics must be hard exit-2 rejections: decorators (dropped
/// would change the checked type), parameter defaults (the skeleton
/// models no default-value evaluation), metaclass keywords (no
/// metaclass modeling) and super() without a method frame.
#[test]
fn subset_rejects_class_constructs() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-reject");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "decorated_class.py",
            "@dec\nclass C:\n    pass\n",
            "decorated classes are outside the skeleton subset",
        ),
        (
            "default_param.py",
            "class C:\n    def m(self, x: int = 1) -> int:\n        return x\n",
            "default parameter values are outside the skeleton subset",
        ),
        (
            "metaclass_keyword.py",
            "class C(metaclass=type):\n    pass\n",
            "metaclass keywords are outside the skeleton subset",
        ),
        (
            "super_outside_method.py",
            "def f(x: int) -> int:\n    return super().m(x)\n",
            "super() outside a method is outside the supported subset",
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

/// Binding-level rules mypy enforces that the subset rejects instead
/// of modeling: a TypeVar call without the typing import (mypy:
/// name-defined), a TypeVar string differing from its variable name
/// (mypy errors at the declaration), and rebinding a module-level name
/// through import / from-import (mypy: no-redef).
#[test]
fn subset_rejects_binding_rules() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-bindings");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "tv_no_import.py",
            "T = TypeVar(\"T\")\n\nclass C:\n    pass\n",
            "TypeVar must be imported from typing",
        ),
        (
            "tv_string_mismatch.py",
            "from typing import TypeVar\nT = TypeVar(\"T2\")\n",
            "a TypeVar string that differs from its variable name",
        ),
        (
            "rebind_import.py",
            "x: int = 1\nimport x\n",
            "rebinding a module-level name",
        ),
        (
            "rebind_from.py",
            "from typing import Generic\nfrom typing import Generic\n",
            "rebinding a module-level name",
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

/// Only `Generic[...]` may subscript a typing marker as a base, and its
/// arguments must be the class's own type variables: without this guard
/// `class C(TypeVar[T])` would be silently modeled as `Generic[T]`, and
/// mypy rejects `class C(Generic[int])` outright.
#[test]
fn marker_base_subscript_requires_generic_and_tvars() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-marker-bases");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "typevar_base.py",
            "from typing import Generic, TypeVar\nT = TypeVar(\"T\")\n\
             class C(TypeVar[T]):\n    pass\n",
            "`TypeVar[...]` as a base class is outside the skeleton subset",
        ),
        (
            "generic_concrete_arg.py",
            "from typing import Generic\nclass C(Generic[int]):\n    pass\n",
            "a `Generic[...]` argument must be a class type variable",
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

/// mypy joins implicitly concatenated string literals before typing them;
/// the AST keeps only the first part, so every string site (TypeVar
/// names, values, annotations) rejects them instead of silently
/// truncating, and star imports get a pointed message of their own.
#[test]
fn subset_rejects_implicit_concat_and_star_imports() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-strings");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "typevar_concat.py",
            "from typing import TypeVar\nT = TypeVar(\"T\" \"x\")\n",
            "implicitly concatenated strings are outside the skeleton subset",
        ),
        (
            "value_concat.py",
            "x: str = \"a\" \"b\"\n",
            "implicitly concatenated strings are outside the skeleton subset",
        ),
        (
            "annotation_concat.py",
            "def f(x: \"int\" \"y\") -> int:\n    return 1\n",
            "implicitly concatenated strings are outside the skeleton subset",
        ),
        (
            "star_import.py",
            "from typing import *\n",
            "star imports are outside the skeleton subset",
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

/// An incompatibility inside an imported sibling must reject with the
/// sibling's path (exit 2), never render as a main-file diagnostic: the
/// renderer owns exactly one path, the main file's.
#[test]
fn sibling_incompatibility_rejects_rather_than_renders() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-sibling");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::write(dir.join("sib.py"), "bad: int = \"x\"\n").expect("write sibling");
    fs::write(dir.join("main.py"), "import sib\n").expect("write main");
    let output = run_bin_in(&dir, "main.py");
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "no stdout for a sibling incompatibility"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sib.py:1"),
        "the sibling's own path: {stderr}"
    );
    assert!(
        stderr.contains("assignment incompatibility is outside the supported error classes"),
        "stderr: {stderr}"
    );
}

/// The override check walks every base definer in the MRO (mypy
/// checker.py:3558), not just the nearest: a chain clean through two
/// definers checks clean, and an incompatible return rejects naming the
/// overriding method's line.
#[test]
fn override_walks_every_definer() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-override");
    fs::create_dir_all(&dir).expect("temp dir");
    let chain = concat!(
        "class A:\n",
        "    def m(self) -> float:\n",
        "        return 1.0\n",
        "\n",
        "\n",
        "class B(A):\n",
        "    def m(self) -> int:\n",
        "        return 1\n",
        "\n",
        "\n",
        "class C(B):\n",
        "    def m(self) -> int:\n",
        "        return 1\n",
    );
    let bad = concat!(
        "class Shape:\n",
        "    def area(self) -> float:\n",
        "        return 0.0\n",
        "\n",
        "\n",
        "class C(Shape):\n",
        "    def area(self) -> str:\n",
        "        return \"\"\n",
    );
    let cases: [(&str, &str, i32, Option<&str>); 2] = [
        ("override_chain_ok.py", chain, 0, None),
        (
            "override_bad_return.py",
            bad,
            2,
            Some("method override incompatibility is outside the supported error classes"),
        ),
    ];
    for (name, source, code, needle) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        match needle {
            Some(n) => assert!(stderr.contains(n), "stderr for {name}: {stderr}"),
            None => assert!(stderr.is_empty(), "stderr for {name}: {stderr}"),
        }
    }
}

/// A class-level annotated assignment overriding a base member (mypy:
/// override errors) is a hard reject, not a silent re-register.
#[test]
fn vardecl_base_member_override_rejected() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-vardecl");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::write(
        dir.join("vardecl_override.py"),
        concat!(
            "class Shape:\n",
            "    kind: str = \"shape\"\n",
            "\n",
            "\n",
            "class C(Shape):\n",
            "    kind: int = 1\n",
        ),
    )
    .expect("write case");
    let output = run_bin_in(&dir, "vardecl_override.py");
    assert_eq!(output.status.code(), Some(2), "exit code");
    assert!(output.stdout.is_empty(), "no stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("overriding a base class member is outside the skeleton subset"),
        "stderr: {stderr}"
    );
}

/// The float/float binop table is explicit: only the vetted operators
/// (Add, Mult, Mod; mypy-clean in the corpus and evidence runs) check
/// as float, and any other operator on floats is rejected at lowering
/// time, never silently tabulated by a wildcard arm.
#[test]
fn float_binop_operator_closure() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-float-binop");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, &str); 4] = [
        (
            "float_add.py",
            "def f(a: float, b: float) -> float:\n    return a + b\n",
            0,
            "",
        ),
        (
            "float_mult.py",
            "def f(a: float, b: float) -> float:\n    return a * b\n",
            0,
            "",
        ),
        (
            "float_mod.py",
            "def f(a: float, b: float) -> float:\n    return a % b\n",
            0,
            "",
        ),
        (
            "float_sub.py",
            "def f(a: float, b: float) -> float:\n    return a - b\n",
            2,
            "binary operator `-` is outside the skeleton subset",
        ),
    ];
    for (name, source, code, expect) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            assert!(output.stdout.is_empty(), "no stdout for {name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expect), "stderr for {name}: {stderr}");
        } else {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "Success: no issues found in 1 source file\n",
                "stdout for {name}"
            );
            assert!(output.stderr.is_empty(), "stderr for {name}");
        }
    }
}

/// Generic constructor inference validates argument-count arity before
/// the parameter/argument zip (#129): a too-short call used to
/// truncate silently and surface the vague inference error, a
/// too-long call fell through to check_call_sig. Both now reject with
/// the arity message, matching real mypy's call-arg errors as an
/// explicit out-of-subset rejection.
#[test]
fn generic_constructor_arity_checked_before_inference() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-generic-arity");
    fs::create_dir_all(&dir).expect("temp dir");
    let class_def = concat!(
        "from typing import Generic, TypeVar\n",
        "T = TypeVar(\"T\")\n",
        "\n",
        "class Box(Generic[T]):\n",
        "    def __init__(self, item: T) -> None:\n",
        "        self.item = item\n",
        "\n",
    );
    let cases = [
        ("generic_too_few.py", "b = Box()\n"),
        ("generic_too_many.py", "b = Box(1, 2)\n"),
    ];
    for (name, tail) in cases {
        let path = dir.join(name);
        fs::write(&path, format!("{class_def}{tail}")).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(output.status.code(), Some(2), "exit code for {name}");
        assert!(output.stdout.is_empty(), "no stdout for {name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("an argument count mismatch is outside the skeleton subset"),
            "stderr for {name}: {stderr}"
        );
    }
}

/// Class-object attribute reads are conservative (#127): a read on a
/// parameterized class object (concrete args via a module var, or a
/// bare generic's frame tvars) rejects instead of leaking an
/// unsubstituted tvar, and a member the class does not define itself
/// (inherited or missing) rejects with its own message. An own
/// non-generic ClassVar read stays supported, as the corpus requires.
#[test]
fn class_object_attr_read_closure() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-classobj-attr");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, &str); 5] = [
        (
            "own_classvar_read.py",
            concat!(
                "class Shape:\n",
                "    kind: str = \"shape\"\n",
                "\n",
                "k: str = Shape.kind\n",
            ),
            0,
            "",
        ),
        (
            "bare_generic_read.py",
            concat!(
                "from typing import Generic, TypeVar\n",
                "T = TypeVar(\"T\")\n",
                "\n",
                "class Box(Generic[T]):\n",
                "    label: str = \"box\"\n",
                "\n",
                "x: str = Box.label\n",
            ),
            2,
            "reading a class variable through a parameterized class object",
        ),
        (
            "module_var_generic_read.py",
            concat!(
                "from typing import Generic, TypeVar\n",
                "T = TypeVar(\"T\")\n",
                "\n",
                "class Box(Generic[T]):\n",
                "    label: str = \"box\"\n",
                "\n",
                "b = Box[int]\n",
                "x: str = b.label\n",
            ),
            2,
            "reading a class variable through a parameterized class object",
        ),
        (
            "inherited_classvar_read.py",
            concat!(
                "class Base:\n",
                "    kind: str = \"base\"\n",
                "\n",
                "class Sub(Base):\n",
                "    pass\n",
                "\n",
                "k: str = Sub.kind\n",
            ),
            2,
            "reading a class variable this class does not define",
        ),
        (
            "method_via_class_object.py",
            concat!(
                "class C:\n",
                "    def m(self) -> None:\n",
                "        pass\n",
                "\n",
                "f = C.m\n",
            ),
            2,
            "reading a non-class-variable through a class object",
        ),
    ];
    for (name, source, code, expect) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            assert!(output.stdout.is_empty(), "no stdout for {name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expect), "stderr for {name}: {stderr}");
        } else {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "Success: no issues found in 1 source file\n",
                "stdout for {name}"
            );
            assert!(output.stderr.is_empty(), "stderr for {name}");
        }
    }
}

/// `__init__` resolution for construction is corpus-only (#131): a
/// class whose corpus MRO defines no `__init__` rejects with the
/// intended message (the fixture walk used to surface builtins.object
/// first), an inherited corpus `__init__` still constructs with the
/// substituted signature, and its arity check matches the own-init path.
#[test]
fn constructor_init_resolution() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-init-resolution");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, &str); 3] = [
        (
            "no_init_construct.py",
            concat!("class C:\n", "    pass\n", "\n", "c = C()\n"),
            2,
            "constructing a class without an __init__ is outside the skeleton subset",
        ),
        (
            "inherited_init.py",
            concat!(
                "class Base:\n",
                "    def __init__(self, x: int) -> None:\n",
                "        self.x = x\n",
                "\n",
                "class Sub(Base):\n",
                "    pass\n",
                "\n",
                "s = Sub(1)\n",
            ),
            0,
            "",
        ),
        (
            "inherited_init_arity.py",
            concat!(
                "class Base:\n",
                "    def __init__(self, x: int) -> None:\n",
                "        self.x = x\n",
                "\n",
                "class Sub(Base):\n",
                "    pass\n",
                "\n",
                "s = Sub(1, 2)\n",
            ),
            2,
            "an argument count mismatch is outside the skeleton subset",
        ),
    ];
    for (name, source, code, expect) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            assert!(output.stdout.is_empty(), "no stdout for {name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expect), "stderr for {name}: {stderr}");
        } else {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "Success: no issues found in 1 source file\n",
                "stdout for {name}"
            );
            assert!(output.stderr.is_empty(), "stderr for {name}");
        }
    }
}

/// Subset rejections name the actual construct kind (#130): the
/// statement/expression kind tables are exhaustive over the parser's
/// AST, so a set literal or a global statement reports its own kind
/// instead of `unsupported`, and a value-position subscript slice gets
/// the value-position message rather than the annotation-flavored one.
#[test]
fn subset_rejection_messages_name_the_construct() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-kind-names");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases = [
        (
            "set_literal.py",
            "x = {1, 2}\n",
            "expression `Set` is outside the skeleton subset",
        ),
        (
            "global_stmt.py",
            "global g\n",
            "statement `Global` is outside the skeleton subset\n",
        ),
        (
            "global_in_body.py",
            "def f() -> None:\n    global g\n    return\n",
            "statement `Global` is outside the skeleton subset in a body\n",
        ),
        (
            "value_subscript.py",
            "items = 5\nx = items[0]\n",
            "subscript arguments in value position must be type expressions: \
             annotation `NumberLiteral` is outside the skeleton subset\n",
        ),
    ];
    for (name, source, needle) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(output.status.code(), Some(2), "exit code for {name}");
        assert!(output.stdout.is_empty(), "no stdout for {name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(needle), "stderr for {name}: {stderr}");
    }
}

/// The int/bool fixture records close the literal promote path: int
/// promotes to float (clean, as in mypy), str(int) is clean, a bool
/// from an int literal renders the exact mypy bytes, and a binop pair
/// the table does not support rejects as out-of-subset (exit 2), never
/// an internal (exit 3).
#[test]
fn int_bool_literal_promote_closure() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-class-slice-promote");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, &str); 4] = [
        ("promote_float.py", "f: float = 1\n", 0, ""),
        ("str_of_int.py", "s: str = str(1)\n", 0, ""),
        (
            "bool_from_int.py",
            "b: bool = 1\n",
            1,
            concat!(
                "bool_from_int.py:1: error: Incompatible types in assignment ",
                "(expression has type \"int\", variable has type \"bool\")  [assignment]\n",
                "Found 1 error in 1 file (checked 1 source file)\n",
            ),
        ),
        (
            "int_binop.py",
            "n = 1 + 1\n",
            2,
            "binary operations on these types are outside the supported subset",
        ),
    ];
    for (name, source, code, expect) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            assert!(output.stdout.is_empty(), "no stdout for {name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expect), "stderr for {name}: {stderr}");
        } else {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if expect.is_empty() {
                assert_eq!(
                    stdout, "Success: no issues found in 1 source file\n",
                    "stdout for {name}"
                );
                assert!(output.stderr.is_empty(), "stderr for {name}");
            } else {
                assert_eq!(stdout, expect, "stdout bytes for {name}");
            }
        }
    }
}

/// mypy exempts `__init__`, `__new__`, `__init_subclass__` and
/// `__post_init__` from the override check unconditionally
/// (checker.py:3547). The `__init_subclass__` and `__post_init__`
/// exemption shapes below are mypy-clean, so the skeleton must accept
/// them; the `__init__`/`__new__` exemptions are exercised by the
/// corpus, and a non-exempt override with a different parameter
/// count still rejects.
#[test]
fn override_exemptions_match_mypy() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-override-exemptions");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, &str); 3] = [
        (
            "init_subclass_override.py",
            concat!(
                "class Base:\n",
                "    def __init_subclass__(self) -> None:\n",
                "        pass\n",
                "\n",
                "class Sub(Base):\n",
                "    def __init_subclass__(self, bad: int) -> None:\n",
                "        pass\n",
            ),
            0,
            "",
        ),
        (
            "post_init_override.py",
            concat!(
                "class Base:\n",
                "    def __post_init__(self, bad: int) -> None:\n",
                "        pass\n",
                "\n",
                "class Sub(Base):\n",
                "    def __post_init__(self) -> None:\n",
                "        pass\n",
            ),
            0,
            "",
        ),
        (
            "plain_override_arity.py",
            concat!(
                "class Base:\n",
                "    def m(self, x: int) -> None:\n",
                "        pass\n",
                "\n",
                "class Sub(Base):\n",
                "    def m(self, x: int, y: int) -> None:\n",
                "        pass\n",
            ),
            2,
            "a method override with a different parameter count",
        ),
    ];
    for (name, source, code, expect) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            assert!(output.stdout.is_empty(), "no stdout for {name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expect), "stderr for {name}: {stderr}");
        } else {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "Success: no issues found in 1 source file\n",
                "stdout for {name}"
            );
            assert!(output.stderr.is_empty(), "stderr for {name}");
        }
    }
}

/// Function and method bodies check after the whole module binds
/// (#126): every name a body reads may be defined later in the
/// module, exactly the set of forward references real mypy accepts
/// (verified against it: each clean case below is mypy-clean). The
/// module-level statements stay eager, so a module assignment that
/// calls a later function keeps rejecting: mypy reports
/// used-before-def there, an error class the subset does not render.
#[test]
fn forward_reference_bodies_check_clean() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-forward-refs");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, &str); 6] = [
        (
            "func_calls_later_func.py",
            concat!(
                "def first() -> int:\n",
                "    return second()\n",
                "\n",
                "def second() -> int:\n",
                "    return 1\n",
            ),
            0,
            "",
        ),
        (
            "method_reads_later_selfattr.py",
            concat!(
                "class C:\n",
                "    def reader(self) -> int:\n",
                "        return self.value\n",
                "\n",
                "    def writer(self) -> None:\n",
                "        self.value = 1\n",
            ),
            0,
            "",
        ),
        (
            "method_calls_later_func.py",
            concat!(
                "class D:\n",
                "    def caller(self) -> int:\n",
                "        return later()\n",
                "\n",
                "def later() -> int:\n",
                "    return 2\n",
            ),
            0,
            "",
        ),
        (
            "func_reads_later_global.py",
            concat!(
                "def global_reader() -> int:\n",
                "    return counter\n",
                "\n",
                "counter: int = 3\n",
            ),
            0,
            "",
        ),
        (
            "later_class_in_method_body.py",
            concat!(
                "class A:\n",
                "    def make(self) -> None:\n",
                "        b = B()\n",
                "\n",
                "class B:\n",
                "    pass\n",
            ),
            2,
            "constructing a class without an __init__ is outside the skeleton subset",
        ),
        (
            "module_assign_stays_eager.py",
            "x = f()\n\ndef f() -> int:\n    return 1\n",
            2,
            "calling `f` is outside the skeleton subset",
        ),
    ];
    for (name, source, code, expect) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            assert!(output.stdout.is_empty(), "no stdout for {name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expect), "stderr for {name}: {stderr}");
        } else {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "Success: no issues found in 1 source file\n",
                "stdout for {name}"
            );
            assert!(output.stderr.is_empty(), "stderr for {name}");
        }
    }
}

/// The manifest's supported forward-reference arm (#126): the
/// differential corpus run must be byte-identical to the .expected
/// fixture the codegen captured from real mypy.
#[test]
fn forward_references_differential() {
    let output = run_bin_in(&testdata_dir(), "forward_references.py");
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = fs::read(testdata_dir().join("forward_references.expected"))
        .expect("missing .expected fixture");
    assert_eq!(output.stdout, expected, "stdout bytes");
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A `self.<attr> = self.<method>()` in the collection sweep must
/// infer the override's return type: the sweep runs after the pass-1
/// members are spliced, so `self.val()` resolves `Derived.val`, not
/// the same-named base method (stale-snapshot regression from the
/// #139 review; mypy infers `Derived.val`'s return and is clean).
#[test]
fn override_selfattr_inference_differential() {
    let output = run_bin_in(&testdata_dir(), "classes_override_selfattr_inference.py");
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = fs::read(testdata_dir().join("classes_override_selfattr_inference.expected"))
        .expect("missing .expected fixture");
    assert_eq!(output.stdout, expected, "stdout bytes");
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A subclass attribute that overrides a base attribute must not be
/// silently kept as a fresh own member when the base's same-named
/// candidate binds only later in the module (the sweep registered it
/// first and first-wins insertion kept it, so an incompatible
/// override passed as `Success`). The provisional own member is
/// dropped after collection, so pass 3 checks the override against the
/// base member and loudly rejects the incompatibility (#93).
#[test]
fn shadowed_base_attr_loudly_rejects() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-shadowed-base");
    fs::create_dir_all(&dir).expect("temp dir");
    let cases: [(&str, &str, i32, Option<&str>); 7] = [
        (
            "later_call.py",
            concat!(
                "class Base:\n",
                "    def __init__(self) -> None:\n",
                "        self.x = later()\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    def __init__(self) -> None:\n",
                "        super().__init__()\n",
                "        self.x = 1\n",
                "\n",
                "\n",
                "def later() -> str:\n",
                "    return \"s\"\n",
            ),
            2,
            Some("instance attribute assignment incompatibility"),
        ),
        (
            "later_class.py",
            concat!(
                "class Base:\n",
                "    def __init__(self) -> None:\n",
                "        self.x = A()\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    def __init__(self) -> None:\n",
                "        super().__init__()\n",
                "        self.x = 1\n",
                "\n",
                "\n",
                "class A:\n",
                "    def __init__(self) -> None:\n",
                "        pass\n",
            ),
            2,
            Some("instance attribute assignment incompatibility"),
        ),
        (
            "clean_override.py",
            concat!(
                "class Base:\n",
                "    def __init__(self) -> None:\n",
                "        self.x = 1\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    def __init__(self) -> None:\n",
                "        super().__init__()\n",
                "        self.x = 2\n",
            ),
            0,
            None,
        ),
        (
            "classvar_shadow.py",
            concat!(
                "class Base:\n",
                "    def __init__(self) -> None:\n",
                "        self.x = later()\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    x: str = \"s\"\n",
                "\n",
                "\n",
                "def later() -> int:\n",
                "    return 1\n",
            ),
            2,
            Some("overriding a base class member"),
        ),
        (
            "classvar_pass1_order.py",
            concat!(
                "class Base:\n",
                "    def __init__(self) -> None:\n",
                "        self.x = 1\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    x: int = 2\n",
            ),
            0,
            None,
        ),
        (
            "classvar_covar_classvar.py",
            concat!(
                "class Base:\n",
                "    x: int = 1\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    x: int = 2\n",
            ),
            0,
            None,
        ),
        (
            "classvar_compat.py",
            concat!(
                "class Base:\n",
                "    def __init__(self) -> None:\n",
                "        self.x = later()\n",
                "\n",
                "\n",
                "class Sub(Base):\n",
                "    x: str = \"s\"\n",
                "\n",
                "\n",
                "def later() -> str:\n",
                "    return \"s\"\n",
            ),
            0,
            None,
        ),
    ];
    for (name, source, expect_code, marker) in cases {
        let path = dir.join(name);
        fs::write(&path, source).expect("write case");
        let output = run_bin_in(&dir, name);
        assert_eq!(
            output.status.code(),
            Some(expect_code),
            "exit code for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let Some(marker) = marker else {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "Success: no issues found in 1 source file\n",
                "stdout for {name}"
            );
            assert!(output.stderr.is_empty(), "stderr for {name}");
            continue;
        };
        assert!(output.stdout.is_empty(), "no stdout for {name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(marker), "stderr for {name}: {stderr}");
    }
}

/// A module value reading a class attribute whose type only settles after
/// the pass-2 repair: mypy keeps the inherited member type, so the module
/// statement is re-typed against the final model and reports the same
/// assignment error mypy does (round-6 review, check.rs:221).
#[test]
fn module_value_retyped_after_shadow_repair() {
    let dir = std::env::temp_dir().join("mypy-rs-skel-retype");
    fs::create_dir_all(&dir).expect("temp dir");
    fs::write(
        dir.join("retype.py"),
        concat!(
            "class Base:\n",
            "    def __init__(self) -> None:\n",
            "        self.x = later()\n",
            "\n",
            "\n",
            "class Sub(Base):\n",
            "    def __init__(self) -> None:\n",
            "        self.x = True\n",
            "\n",
            "\n",
            "def later() -> int:\n",
            "    return 1\n",
            "\n",
            "\n",
            "s = Sub()\n",
            "n: bool = s.x\n",
        ),
    )
    .expect("write case");
    let output = run_bin_in(&dir, "retype.py");
    assert_eq!(output.status.code(), Some(1), "exit code");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "Incompatible types in assignment (expression has type \"int\", \
             variable has type \"bool\")"
        ),
        "stdout: {stdout}"
    );
}
