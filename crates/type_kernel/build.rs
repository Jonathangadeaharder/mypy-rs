fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EXTENSION_MODULE");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // pyo3 0.20 never emits the macOS cdylib link args itself (the helper
    // in pyo3-build-config is uncalled), so a cdylib with the feature on
    // fails on undefined Py_* without these. Feature off: no change.
    if std::env::var("CARGO_FEATURE_EXTENSION_MODULE").is_ok() && target_os == "macos" {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
