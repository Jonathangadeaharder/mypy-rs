fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        // The bin links the type_kernel rlib built with pyo3's
        // extension-module feature (no libpython link directive): the
        // stubs satisfy its Py_* refs; a new one must fail the link.
        println!("cargo:rustc-link-arg=-Wl,-dead_strip");
    }
}
