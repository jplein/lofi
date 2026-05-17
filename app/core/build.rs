// Cargo build script.
//
// The Bazel build runs cbindgen via a `genrule` in `BUILD.bazel`, not
// via this script — Bazel does not execute `build.rs` for the
// top-level crate. This file exists so cbindgen remains a referenced
// `[build-dependencies]` entry in `Cargo.lock`; without that, crate
// resolution would drop it and Bazel's `@crates//:cbindgen__cli` target
// would disappear.
//
// As a side benefit, building `lofi-core --features ffi` via Cargo
// directly (outside Bazel) still produces `include/lofi_core.h` so a
// non-Bazel consumer can use the FFI surface.

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    // Only the `ffi` feature pulls in the header generation. The
    // GNOME-side Cargo build (which does not set this feature) is a
    // pure no-op.
    if std::env::var("CARGO_FEATURE_FFI").is_err() {
        return;
    }

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out_dir = std::path::Path::new(&crate_dir).join("include");
    std::fs::create_dir_all(&out_dir).expect("create include/ dir");

    let config = cbindgen::Config::from_file(format!("{crate_dir}/cbindgen.toml"))
        .expect("read cbindgen.toml");

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("cbindgen generate")
        .write_to_file(out_dir.join("lofi_core.h"));
}
