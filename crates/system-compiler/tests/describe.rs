//! The CLI: `describe` on compiled keys and on files, `max-depth`, and
//! `compile` agreeing with the library in a fresh process.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::ORIGIN_UTC_MS;
use gx_core::sha256::{sha256, to_hex};
use serde_json::Value;
use system_compiler::{compile, CompileOptions, System};

const BIN: &str = env!("CARGO_BIN_EXE_system-compiler");

/// Writes `bytes` to a file unique to this test and returns its path.
fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("describe-{name}.bin"));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Runs the binary with `args` and returns its stdout.
fn run(args: &[&str]) -> Vec<u8> {
    let out = Command::new(BIN).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn json(args: &[&str]) -> Value {
    serde_json::from_slice(&run(args)).unwrap()
}

#[test]
fn from_file_matches_compiling_the_key() {
    let key = "4-1-0-0-0";
    let bytes = compile(
        &System::bundled(),
        &key.parse().unwrap(),
        0,
        &CompileOptions::default(),
    )
    .unwrap();
    let path = write_temp("bare-cell", &bytes);
    let from_file = json(&[
        "describe",
        "--key",
        key,
        "--from-file",
        path.to_str().unwrap(),
    ]);
    assert_eq!(from_file, json(&["describe", "--key", key]));
    assert_eq!(from_file["empty"], false);
    assert_eq!(from_file["sha256"], to_hex(&sha256(&bytes)));
}

#[test]
fn describe_registry_at_an_origin() {
    let v = json(&[
        "describe",
        "--key",
        "registry",
        "--origin",
        "2026-11-14T00:00:00Z",
    ]);
    assert_eq!(v["frame_count"], 11);
    let reg = System::bundled().registry_at(ORIGIN_UTC_MS).unwrap();
    assert_eq!(v["epoch_s"], reg.epoch().value());
    assert_eq!(v["frames"][4]["j2"], reg.frames()[4].j2);
    // Without --origin the registry is at the data epoch, rounded to the
    // nearest UTC millisecond.
    let epoch = json(&["describe", "--key", "registry"])["epoch_s"]
        .as_f64()
        .unwrap();
    assert!(epoch.abs() < 1e-3, "{epoch}");
}

#[test]
fn compile_in_a_fresh_process_matches_the_library() {
    let s = System::bundled();
    for (key, origin) in [("registry", ORIGIN_UTC_MS), ("6-2-1-2-1", ORIGIN_UTC_MS)] {
        let bytes = compile(
            &s,
            &key.parse().unwrap(),
            origin,
            &CompileOptions::default(),
        )
        .unwrap();
        let origin_arg = origin.to_string();
        let out = run(&["compile", "--key", key, "--origin", &origin_arg]);
        assert_eq!(out, bytes, "{key}");
    }
}

#[test]
fn max_depth_table_lists_every_frame() {
    let v = json(&["max-depth"]);
    let table = v.as_object().unwrap();
    assert_eq!(table.len(), 11);
    assert_eq!(table["0"], 0);
    for id in 1..=10 {
        assert_eq!(table[&id.to_string()], 6);
    }
}

#[test]
fn bad_origins_are_refused() {
    let out = Command::new(BIN)
        .args(["compile", "--key", "registry", "--origin", "tomorrow"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}
