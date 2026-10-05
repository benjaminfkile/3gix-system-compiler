//! Golden hashes: the SHA-256 of the compiled bytes of a fixed list of keys
//! must match `tests/golden.json`.
//!
//! Any change to `data/system.toml`, the resolution table, the sub-sampling
//! scheme, the compression, or the gx-core version that changes a hash is a
//! golden change: it changes what the hub stores and must be reviewed as
//! such. Regenerate with `UPDATE_GOLDEN=1 cargo test -p system-compiler
//! --test golden`, then review and commit the diff of `tests/golden.json`
//! together with the change that caused it (see `docs/compile.md`).
//!
//! The same keys are also compiled twice in this process and once in a
//! fresh process through the binary, and every result must be identical
//! and pass `gx_core::validate::validate`.

mod common;

use std::collections::BTreeMap;
use std::process::Command;

use common::system;
use gx_core::key::ChunkKey;
use sha2::{Digest, Sha256};
use system_compiler::{compile, CompileOptions};

/// The golden keys and why each is on the list.
const KEYS: [(&str, &str); 7] = [
    ("registry", "the frame registry"),
    ("4-0-0-0-0", "frame 4 at depth 0, the whole body"),
    (
        "4-2-1-1-1",
        "frame 4 at depth 2, a cell holding part of the surface",
    ),
    ("1-1-0-0-0", "frame 1 at depth 1"),
    ("10-3-3-3-3", "frame 10 at depth 3"),
    (
        "6-2-0-0-0",
        "frame 6 at depth 2, a far corner cell that is empty",
    ),
    ("0-0-0-0-0", "the root frame, which holds no matter"),
];

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden.json");

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn compile_key(key: &str) -> Vec<u8> {
    let s = system();
    let k: ChunkKey = key.parse().unwrap();
    compile(&s, &k, &CompileOptions::default()).unwrap()
}

#[test]
fn hashes_match_golden() {
    let actual: BTreeMap<String, String> = KEYS
        .iter()
        .map(|(k, _)| (k.to_string(), sha256_hex(&compile_key(k))))
        .collect();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let text = serde_json::to_string_pretty(&actual).unwrap() + "\n";
        std::fs::write(GOLDEN, text).unwrap();
    }
    let text = std::fs::read_to_string(GOLDEN).expect("tests/golden.json exists");
    let expected: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
    assert_eq!(
        expected.keys().collect::<Vec<_>>(),
        actual.keys().collect::<Vec<_>>(),
        "golden key list changed"
    );
    for (key, why) in KEYS {
        assert_eq!(
            actual[key], expected[key],
            "golden change for {key} ({why}): review it, then regenerate"
        );
    }
}

#[test]
fn golden_sections_validate() {
    for (key, _) in KEYS {
        let bytes = compile_key(key);
        gx_core::validate::validate(key, &bytes).unwrap_or_else(|e| panic!("{key}: {e}"));
    }
}

#[test]
fn golden_shapes() {
    let empty = |key: &str| {
        let ChunkKey::Cell(c) = key.parse().unwrap() else {
            unreachable!()
        };
        gx_core::matter::decode(&c, &compile_key(key))
            .unwrap()
            .is_empty()
    };
    assert!(!empty("4-0-0-0-0"));
    assert!(!empty("4-2-1-1-1"));
    assert!(!empty("1-1-0-0-0"));
    assert!(!empty("10-3-3-3-3"));
    assert!(empty("6-2-0-0-0"));
    assert!(empty("0-0-0-0-0"));
    let reg = gx_core::registry::decode(&compile_key("registry")).unwrap();
    assert_eq!(reg.frames().len(), 11);
}

#[test]
fn identical_twice_in_process() {
    for (key, _) in KEYS {
        assert_eq!(compile_key(key), compile_key(key), "{key}");
    }
}

#[test]
fn identical_in_a_fresh_process() {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/system.toml");
    for (key, _) in KEYS {
        let out = Command::new(env!("CARGO_BIN_EXE_system-compiler"))
            .args(["compile", "--data", data, "--key", key])
            .output()
            .expect("the binary runs");
        assert!(
            out.status.success(),
            "{key}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bytes = compile_key(key);
        assert_eq!(out.stdout, bytes, "{key}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.contains(&sha256_hex(&bytes)) && stderr.contains(&bytes.len().to_string()),
            "{key}: {stderr}"
        );
    }
}
