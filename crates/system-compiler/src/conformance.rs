//! The module's conformance vectors: fixed requests and the SHA-256 of the
//! section each must produce.
//!
//! Implements the determinism check of `space-model.md` section 4 item 6
//! for this module: `conformance/` holds, per vector, `NAME.request.bin`
//! (the request bytes of `client-compute.md` section 4), `NAME.tiles.bin`
//! (the empty tile list: this module reads no sources), and `NAME.sha256`
//! (lowercase hex of the expected section), plus `index.json` listing them.
//! The files are the input of the core determinism harness
//! (`scripts/module-determinism.sh` in the core repository), which runs the
//! built module natively and under Node and requires these hashes from
//! both. [`write`] regenerates the directory by running the module's logic
//! natively ([`gx_module_guest::run_compile`]).

use std::fmt::Write as _;
use std::path::Path;

use gx_core::module_io::{encode_request, encode_tiles, ModuleRequest};
use gx_core::sha256::{sha256, to_hex};
use gx_module_guest::run_compile;

use crate::SystemModule;

/// 2026-11-14T00:00:00Z in UTC milliseconds: the origin the registry test
/// checks against the published ephemeris.
pub const ORIGIN_A_UTC_MS: i64 = 1_794_614_400_000;

/// 2026-11-15T12:00:00Z in UTC milliseconds: a second origin.
pub const ORIGIN_B_UTC_MS: i64 = 1_794_744_000_000;

/// One conformance vector.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// File name stem.
    pub name: &'static str,
    /// The request's origin, UTC milliseconds.
    pub origin_utc_ms: i64,
    /// The chunk key.
    pub key: &'static str,
    /// What the vector covers.
    pub why: &'static str,
}

/// The fifteen vectors: the registry at two origins, cells of several
/// bodies at several depths, and three frame keys (`matter-format.md`
/// section 11) at origin 0, as the hub sends them: a body with moments, the
/// root, and a frame the system does not declare.
pub const VECTORS: [Vector; 15] = [
    v(
        "01-registry-a",
        ORIGIN_A_UTC_MS,
        "registry",
        "registry at origin A",
    ),
    v(
        "02-registry-b",
        ORIGIN_B_UTC_MS,
        "registry",
        "registry at origin B",
    ),
    v(
        "03-plasma-d0",
        ORIGIN_A_UTC_MS,
        "1-0-0-0-0",
        "the plasma polytrope whole, depth 0",
    ),
    v(
        "04-plasma-d2",
        ORIGIN_A_UTC_MS,
        "1-2-1-1-1",
        "the plasma core octant, depth 2",
    ),
    v(
        "05-solid-d1",
        ORIGIN_A_UTC_MS,
        "4-1-1-1-1",
        "a uniform solid ball octant, depth 1",
    ),
    v(
        "06-solid-d3",
        ORIGIN_B_UTC_MS,
        "4-3-3-4-3",
        "a uniform solid surface cell, depth 3",
    ),
    v(
        "07-gas-d2",
        ORIGIN_A_UTC_MS,
        "6-2-1-2-1",
        "a gas polytrope octant, depth 2",
    ),
    v(
        "08-gas-d4",
        ORIGIN_A_UTC_MS,
        "7-4-7-8-7",
        "a gas polytrope interior cell, depth 4",
    ),
    v(
        "09-solid-d5",
        ORIGIN_A_UTC_MS,
        "10-5-15-16-15",
        "the orbiting solid, depth 5",
    ),
    v(
        "10-gas-d6",
        ORIGIN_A_UTC_MS,
        "9-6-31-32-32",
        "a gas polytrope center cell, depth 6",
    ),
    v(
        "11-root",
        ORIGIN_A_UTC_MS,
        "0-0-0-0-0",
        "the root frame, which holds no matter",
    ),
    v(
        "12-empty",
        ORIGIN_A_UTC_MS,
        "3-2-0-0-0",
        "a far corner cell that is empty",
    ),
    v(
        "13-frame-body",
        0,
        "frame-10",
        "the frame record of the orbiting solid, with moments",
    ),
    v(
        "14-frame-root",
        0,
        "frame-0",
        "the frame record of the root",
    ),
    v(
        "15-frame-undeclared",
        0,
        "frame-42",
        "a frame the system does not declare: the empty record",
    ),
];

const fn v(name: &'static str, origin_utc_ms: i64, key: &'static str, why: &'static str) -> Vector {
    Vector {
        name,
        origin_utc_ms,
        key,
        why,
    }
}

impl Vector {
    /// The request bytes.
    pub fn request(&self) -> Vec<u8> {
        encode_request(&ModuleRequest {
            origin_utc_ms: self.origin_utc_ms,
            span_seconds: 0,
            span_index: 0,
            key: self.key.parse().expect("conformance keys parse"),
        })
    }

    /// The tile bytes: an empty list.
    pub fn tiles(&self) -> Vec<u8> {
        encode_tiles(&[]).expect("an empty tile list encodes")
    }

    /// The section the module returns, computed natively.
    pub fn section(&self) -> Result<Vec<u8>, String> {
        run_compile::<SystemModule>(&self.request(), &self.tiles())
    }
}

/// Every file of the conformance directory, `(file name, bytes)`, in name
/// order.
pub fn files() -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut index = String::from("{\n  \"cases\": {\n");
    for (i, vec) in VECTORS.iter().enumerate() {
        let hash = to_hex(&sha256(&vec.section()?));
        out.push((format!("{}.request.bin", vec.name), vec.request()));
        out.push((
            format!("{}.sha256", vec.name),
            format!("{hash}\n").into_bytes(),
        ));
        out.push((format!("{}.tiles.bin", vec.name), vec.tiles()));
        let comma = if i + 1 < VECTORS.len() { "," } else { "" };
        let _ = write!(
            index,
            "    \"{}\": {{\n      \"key\": \"{}\",\n      \"origin_utc_ms\": {},\n      \"sha256\": \"{}\",\n      \"sources\": [],\n      \"why\": \"{}\"\n    }}{comma}\n",
            vec.name, vec.key, vec.origin_utc_ms, hash, vec.why
        );
    }
    index.push_str("  }\n}\n");
    out.push(("index.json".to_string(), index.into_bytes()));
    out.sort();
    Ok(out)
}

/// Writes every file of [`files`] into `dir`, creating it if needed.
pub fn write(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, bytes) in files()? {
        let p = dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}
