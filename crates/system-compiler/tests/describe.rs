//! The `describe` subcommand on files: a bare section, a bare registry, and
//! hub containers (`matter-format.md` section 6) built with gx-core's
//! `encode_chunk`, which writes the same bytes the hub does.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{system, EARTH};
use gx_core::container::encode_chunk;
use gx_core::key::{CellKey, ChunkKey};
use serde_json::Value;
use system_compiler::{compile, CompileOptions};

/// Writes `bytes` to a file unique to this test and returns its path.
fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("describe-{name}.bin"));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Runs `system-compiler describe` with `args` and parses its JSON output.
fn describe(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_system-compiler"))
        .arg("describe")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "describe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn compiled(key: &ChunkKey) -> Vec<u8> {
    compile(&system(), key, &CompileOptions::default()).unwrap()
}

#[test]
fn from_file_matches_compiling_the_key() {
    let key = "4-1-0-0-0";
    let bytes = compiled(&key.parse().unwrap());
    let path = write_temp("bare-cell", &bytes);
    let from_file = describe(&["--key", key, "--from-file", path.to_str().unwrap()]);
    assert_eq!(from_file, describe(&["--key", key]));
}

#[test]
fn container_with_one_cell_section() {
    let cell = CellKey {
        frame_id: EARTH,
        depth: 1,
        x: 1,
        y: 0,
        z: 1,
    };
    let bare = compiled(&ChunkKey::Cell(cell));
    let path = write_temp("container-cell", &encode_chunk(&[&bare], &["layer"]));
    let key = cell.to_string();
    let v = describe(&[
        "--key",
        &key,
        "--from-file",
        path.to_str().unwrap(),
        "--container",
    ]);
    let section = describe(&["--key", &key]);
    assert_eq!(v["section_count"], 1);
    assert_eq!(v["sections"][0]["empty"], false);
    assert_eq!(v["mass_kg"], section["mass_kg"]);
    assert_eq!(v["sections"][0]["mass_kg"], section["mass_kg"]);
}

#[test]
fn container_with_one_empty_section() {
    let key = "4-2-0-0-0";
    let bare = compiled(&key.parse().unwrap());
    let path = write_temp("container-empty", &encode_chunk(&[&bare], &["layer"]));
    let v = describe(&[
        "--key",
        key,
        "--from-file",
        path.to_str().unwrap(),
        "--container",
    ]);
    assert_eq!(v["section_count"], 1);
    assert_eq!(v["sections"][0]["empty"], true);
    assert_eq!(v["mass_kg"], 0.0);
}

#[test]
fn container_with_the_registry() {
    let bare = compiled(&ChunkKey::Registry);
    let path = write_temp("container-registry", &encode_chunk(&[&bare], &["layer"]));
    let v = describe(&[
        "--key",
        "registry",
        "--from-file",
        path.to_str().unwrap(),
        "--container",
    ]);
    assert_eq!(v["section_count"], 1);
    assert_eq!(v["frame_count"], 11);
    assert_eq!(v["sections"][0]["frame_count"], 11);
}

#[test]
fn bare_bytes_are_not_a_container() {
    let bare = compiled(&ChunkKey::Registry);
    let path = write_temp("not-a-container", &bare);
    let out = Command::new(env!("CARGO_BIN_EXE_system-compiler"))
        .args(["describe", "--key", "registry", "--from-file"])
        .arg(&path)
        .arg("--container")
        .output()
        .unwrap();
    assert!(!out.status.success());
}
