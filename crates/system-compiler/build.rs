//! Linker arguments every module needs on `wasm32` (core `docs/modules.md`).
//!
//! The standard library for `wasm32-unknown-unknown` is compiled with the
//! reference-types feature, so its `call_indirect` instructions carry their
//! table index as a padded five-byte LEB. A WebAssembly 1.0 validator, which
//! the module host is (`client-compute.md` section 4), only accepts a single
//! zero byte there. `--compress-relocations` makes the linker write minimal
//! LEBs, and it requires `--strip-debug`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=--compress-relocations");
        println!("cargo:rustc-link-arg-cdylib=--strip-debug");
    }
}
