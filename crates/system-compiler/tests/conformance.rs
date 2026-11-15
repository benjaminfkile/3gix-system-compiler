//! The conformance vectors in `conformance/` are what the module computes.
//!
//! The vectors pin the module's output bytes (`space-model.md` section 4
//! item 6). This test regenerates them natively through the module's own
//! entry point (`gx_module_guest::run_compile`) and requires every file to
//! match the checked-in copy byte for byte; `scripts/ci.sh` then runs the
//! same vectors through the built WebAssembly module natively and under
//! Node with the core determinism harness. To refresh the vectors after an
//! intended change, run
//! `cargo run -p system-compiler --release -- conformance --out conformance`
//! and review the diff (see `docs/compile.md`).

use std::path::Path;

use gx_core::module_io::decode_source_ids;
use gx_module_guest::run_sources;
use system_compiler::conformance::{files, VECTORS};
use system_compiler::{compile, CompileOptions, System, SystemModule};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../conformance");

#[test]
fn checked_in_vectors_match() {
    let dir = Path::new(DIR);
    let generated = files().unwrap();
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    on_disk.sort();
    let names: Vec<&String> = generated.iter().map(|(n, _)| n).collect();
    assert_eq!(on_disk.iter().collect::<Vec<_>>(), names);
    for (name, bytes) in &generated {
        let disk = std::fs::read(dir.join(name)).unwrap();
        assert!(
            &disk == bytes,
            "conformance/{name} differs from what the module computes"
        );
    }
}

#[test]
fn twelve_vectors_with_two_registries() {
    assert_eq!(VECTORS.len(), 12);
    let registries: Vec<i64> = VECTORS
        .iter()
        .filter(|v| v.key == "registry")
        .map(|v| v.origin_utc_ms)
        .collect();
    assert_eq!(registries.len(), 2);
    assert_ne!(registries[0], registries[1]);
}

#[test]
fn the_module_is_the_library() {
    let s = System::bundled();
    for v in VECTORS.iter().filter(|v| v.key != "registry") {
        let direct = compile(
            &s,
            &v.key.parse().unwrap(),
            v.origin_utc_ms,
            &CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(v.section().unwrap(), direct, "{}", v.name);
        let ids = run_sources::<SystemModule>(&v.request()).unwrap();
        assert!(decode_source_ids(&ids).unwrap().is_empty());
    }
}
