//! Build script for `lofi-core`.
//!
//! Generates `include/lofi_core.h` via cbindgen when the `ffi` feature is on.
//! With the feature off this script is a no-op so the GNOME build (and the
//! default workspace build) doesn't depend on cbindgen at all.
//!
//! The header is gitignored — the Rust signatures in `src/ffi/` are the
//! source of truth.
//!
//! Linking note: integration tests in `tests/ffi.rs` reach the FFI symbols
//! through `extern "C"` declarations, not through any Rust path in
//! `lofi_core::*`. They make sure the rlib stays on the linker's input list
//! by writing `extern crate lofi_core as _;` at the top of the test file —
//! so this build script does not need to spawn a nested staticlib build or
//! emit `rustc-link-arg-tests` directives.

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    // The `ffi` Cargo feature sets `CARGO_FEATURE_FFI=1` in the build-script
    // environment. When unset we emit nothing — important so the GNOME side
    // and the default `cargo test -p lofi-core` invocation stay
    // cbindgen-free.
    if std::env::var_os("CARGO_FEATURE_FFI").is_none() {
        return;
    }

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set by cargo for build scripts");
    let crate_dir = std::path::PathBuf::from(crate_dir);
    let include_dir = crate_dir.join("include");
    std::fs::create_dir_all(&include_dir)
        .expect("creating app/core/include/ for the generated C header");
    let header_path = include_dir.join("lofi_core.h");

    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
        .expect("reading cbindgen.toml");

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("cbindgen failed to generate lofi_core.h")
        .write_to_file(&header_path);
}
