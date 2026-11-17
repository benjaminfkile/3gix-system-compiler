//! `system-compiler`: the native command line for local work on the module.
//!
//! It calls the same functions the WebAssembly module runs
//! ([`system_compiler::compile()`]), so what it prints is what the module
//! returns. Subcommands:
//!
//! - `compile`: compile one chunk key at an origin to bytes (file or
//!   stdout), reporting the byte length and SHA-256 on stderr.
//! - `describe`: compile one chunk key, or read bytes from a file with
//!   `--from-file`, decode them with gx-core, and print a JSON summary of
//!   the section or the registry.
//! - `keys`: list every cell key of a frame at one depth whose section is
//!   not empty, one per line, sorted by `(x, y, z)`.
//! - `frames`: list the frames of the registry with their compiler-side
//!   names, for humans.
//! - `max-depth`: print the layer config's `maxDepth` table
//!   (`client-compute.md` section 2.2) as JSON, from the registry.
//! - `conformance`: regenerate the conformance vectors.
//!
//! Every subcommand except `conformance` reads the system from `--data`, or
//! from the data baked into the binary when `--data` is omitted. `--origin`
//! takes UTC milliseconds or `YYYY-MM-DDTHH:MM:SS[.mmm]Z`; it defaults to
//! the data epoch.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use gx_core::key::{CellKey, ChunkKey};
use gx_core::matter::{Compression, Section};
use gx_core::registry::ROOT_PARENT;
use gx_core::sha256::{sha256, to_hex};
use gx_core::timescale::{tdb_seconds_to_utc_ms, utc_ms_of_date};
use serde_json::{json, Value};
use system_compiler::compile::{compile, non_empty_keys, CompileOptions};
use system_compiler::System;

/// The system module's native command line.
#[derive(Parser)]
#[command(name = "system-compiler", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile one chunk key and write its bytes.
    Compile {
        /// System data file; the bundled data when omitted.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Chunk key: `registry` or `frameId-depth-x-y-z`.
        #[arg(long)]
        key: String,
        /// The universe's origin: UTC milliseconds or
        /// `YYYY-MM-DDTHH:MM:SS[.mmm]Z`. The data epoch when omitted.
        #[arg(long)]
        origin: Option<String>,
        /// Output file; stdout when omitted.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Leave the sample block uncompressed.
        #[arg(long)]
        no_compress: bool,
    },
    /// Compile one chunk key, or read bytes from a file, and print a JSON
    /// summary of the decoded result.
    Describe {
        /// System data file; the bundled data when omitted. Unused with
        /// `--from-file`.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Chunk key: `registry` or `frameId-depth-x-y-z`.
        #[arg(long)]
        key: String,
        /// The universe's origin, as for `compile`.
        #[arg(long)]
        origin: Option<String>,
        /// Describe the bytes in this file instead of compiling the key.
        #[arg(long)]
        from_file: Option<PathBuf>,
    },
    /// List the cell keys at one depth of a frame whose section is not empty.
    Keys {
        /// System data file; the bundled data when omitted.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Frame id.
        #[arg(long)]
        frame: u64,
        /// Cell depth, 0 to 31.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=31))]
        depth: u8,
    },
    /// List the registry's frames at an origin with their compiler-side
    /// names.
    Frames {
        /// System data file; the bundled data when omitted.
        #[arg(long)]
        data: Option<PathBuf>,
        /// The universe's origin, as for `compile`.
        #[arg(long)]
        origin: Option<String>,
    },
    /// Print the layer's `maxDepth` table as JSON: every frame id with the
    /// registry's `max_depth`.
    MaxDepth {
        /// System data file; the bundled data when omitted.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// Regenerate the conformance vectors into a directory.
    Conformance {
        /// Output directory.
        #[arg(long, default_value = "conformance")]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("system-compiler: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn load(data: Option<&PathBuf>) -> anyhow::Result<System> {
    match data {
        None => Ok(System::bundled()),
        Some(p) => System::load(p).with_context(|| format!("loading {}", p.display())),
    }
}

fn parse_key(key: &str) -> anyhow::Result<ChunkKey> {
    key.parse::<ChunkKey>()
        .map_err(|e| anyhow::anyhow!("key {key:?}: {e}"))
}

/// The origin in UTC milliseconds: `--origin` parsed, or the data epoch.
fn origin(system: &System, text: Option<&str>) -> anyhow::Result<i64> {
    match text {
        None => Ok(tdb_seconds_to_utc_ms(system.epoch().value())),
        Some(t) => parse_origin(t),
    }
}

/// Parses UTC milliseconds, or `YYYY-MM-DDTHH:MM:SS[.mmm]Z`.
fn parse_origin(text: &str) -> anyhow::Result<i64> {
    if let Ok(ms) = text.parse::<i64>() {
        return Ok(ms);
    }
    let bad = || anyhow::anyhow!("origin {text:?}: not UTC milliseconds or YYYY-MM-DDTHH:MM:SSZ");
    let t = text.strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = t.split_once('T').ok_or_else(bad)?;
    let num = |s: &str| s.parse::<i64>().map_err(|_| bad());
    let d: Vec<&str> = date.split('-').collect();
    let h: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || h.len() != 3 {
        return Err(bad());
    }
    let (sec, ms) = match h[2].split_once('.') {
        Some((s, f)) if f.len() == 3 => (num(s)?, num(f)?),
        Some(_) => return Err(bad()),
        None => (num(h[2])?, 0),
    };
    let (year, month, day) = (num(d[0])?, num(d[1])?, num(d[2])?);
    let (hour, minute) = (num(h[0])?, num(h[1])?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&sec)
    {
        return Err(bad());
    }
    Ok(utc_ms_of_date(year, month, day) + ((hour * 60 + minute) * 60 + sec) * 1000 + ms)
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Compile {
            data,
            key,
            origin: o,
            out,
            no_compress,
        } => {
            let system = load(data.as_ref())?;
            let key = parse_key(&key)?;
            let origin_ms = origin(&system, o.as_deref())?;
            let opts = CompileOptions {
                compression: if no_compress {
                    Compression::None
                } else {
                    Compression::Zstd
                },
                ..CompileOptions::default()
            };
            let bytes = compile(&system, &key, origin_ms, &opts)
                .with_context(|| format!("compiling {key}"))?;
            match out {
                Some(path) => std::fs::write(&path, &bytes)
                    .with_context(|| format!("writing {}", path.display()))?,
                None => {
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
            }
            eprintln!("{} bytes sha256 {}", bytes.len(), to_hex(&sha256(&bytes)));
        }
        Command::Describe {
            data,
            key,
            origin: o,
            from_file,
        } => {
            let key = parse_key(&key)?;
            let bytes = match &from_file {
                Some(path) => {
                    std::fs::read(path).with_context(|| format!("reading {}", path.display()))?
                }
                None => {
                    let system = load(data.as_ref())?;
                    let origin_ms = origin(&system, o.as_deref())?;
                    compile(&system, &key, origin_ms, &CompileOptions::default())
                        .with_context(|| format!("compiling {key}"))?
                }
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&describe(&key, &bytes)?)?
            );
        }
        Command::Keys { data, frame, depth } => {
            let system = load(data.as_ref())?;
            let mut stdout = std::io::stdout().lock();
            for key in non_empty_keys(&system, frame, depth)? {
                writeln!(stdout, "{key}")?;
            }
        }
        Command::Frames { data, origin: o } => {
            let system = load(data.as_ref())?;
            let origin_ms = origin(&system, o.as_deref())?;
            frames(&system, origin_ms)?;
        }
        Command::MaxDepth { data } => {
            let system = load(data.as_ref())?;
            let table: serde_json::Map<String, Value> = system
                .epoch_registry()
                .frames()
                .iter()
                .map(|f| (f.frame_id.to_string(), json!(f.max_depth)))
                .collect();
            println!("{}", Value::Object(table));
        }
        Command::Conformance { out } => {
            system_compiler::conformance::write(&out).map_err(anyhow::Error::msg)?;
            eprintln!("wrote {}", out.display());
        }
    }
    Ok(())
}

/// Decodes one section or registry with gx-core and summarizes it as JSON.
fn describe(key: &ChunkKey, bytes: &[u8]) -> anyhow::Result<Value> {
    let mut summary = match key {
        ChunkKey::Registry => registry_json(&gx_core::registry::decode(bytes)?),
        ChunkKey::Cell(cell) => section_json(cell, &gx_core::matter::decode(cell, bytes)?),
        ChunkKey::Surface(tile) => {
            gx_core::surface::decode(tile, bytes)?;
            json!({ "surface": "empty" })
        }
    };
    summary["key"] = json!(key.to_string());
    summary["bytes"] = json!(bytes.len());
    summary["sha256"] = json!(to_hex(&sha256(bytes)));
    Ok(summary)
}

/// JSON summary of one decoded registry: epoch and every frame.
fn registry_json(reg: &gx_core::registry::Registry) -> Value {
    let frames: Vec<Value> = reg
        .frames()
        .iter()
        .map(|f| {
            let q = f.orientation;
            let m = f.principal_moments;
            json!({
                "frame_id": f.frame_id,
                "parent_frame_id": (f.parent_frame_id != ROOT_PARENT)
                    .then_some(f.parent_frame_id),
                "root_extent_m": f.root_extent.value(),
                "max_depth": f.max_depth,
                "mass_kg": f.mass.value(),
                "position_m": [f.position.x, f.position.y, f.position.z],
                "velocity_m_per_s": [f.velocity.x, f.velocity.y, f.velocity.z],
                "orientation_xyzw": [q.x, q.y, q.z, q.w],
                "angular_velocity_rad_per_s":
                    [f.angular_velocity.x, f.angular_velocity.y, f.angular_velocity.z],
                "j2": f.j2,
                "reference_radius_m": f.reference_radius.value(),
                "principal_moments_kg_m2": [m.x, m.y, m.z],
            })
        })
        .collect();
    json!({
        "epoch_s": reg.epoch().value(),
        "frame_count": frames.len(),
        "frames": frames,
    })
}

/// JSON summary of one decoded matter section of `cell`.
fn section_json(cell: &CellKey, s: &Section) -> Value {
    let non_vacuum = s.samples().map_or(0, |samples| {
        samples.iter().filter(|x| x.density.value() > 0.0).count()
    });
    let o = s.origin();
    json!({
        "frame_id": cell.frame_id,
        "depth": cell.depth,
        "cell": [cell.x, cell.y, cell.z],
        "cell_origin_m": [o.x, o.y, o.z],
        "cell_edge_m": s.edge().value(),
        "empty": s.is_empty(),
        "resolution": s.resolution(),
        "samples": s.samples().map_or(0, |x| x.len()),
        "non_vacuum": non_vacuum,
        "mass_kg": s.mass().value(),
    })
}

/// Prints one line per registry frame at the origin, with compiler-side
/// names.
fn frames(system: &System, origin_ms: i64) -> anyhow::Result<()> {
    let registry = system.registry_at(origin_ms)?;
    println!(
        "epoch {} s TDB since J2000, {} frames",
        registry.epoch().value(),
        registry.frames().len()
    );
    for f in registry.frames() {
        let body = system.body(f.frame_id);
        let name = body.map_or(system.root().name.as_str(), |b| b.name.as_str());
        let density = body.map_or(0.0, |b| b.mean_density().value());
        println!(
            "{:>3} {:<26} mass {:.6e} kg  distance {:.6e} m  extent {:.4e} m  depth {}  mean density {:.1} kg/m^3",
            f.frame_id,
            name,
            f.mass.value(),
            f.position.length(),
            f.root_extent.value(),
            f.max_depth,
            density
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_origin;

    #[test]
    fn origins_parse() {
        assert_eq!(parse_origin("1794614400000").unwrap(), 1_794_614_400_000);
        assert_eq!(
            parse_origin("2026-11-14T00:00:00Z").unwrap(),
            1_794_614_400_000
        );
        assert_eq!(
            parse_origin("2026-11-15T12:00:00.250Z").unwrap(),
            1_794_744_000_250
        );
        assert_eq!(parse_origin("1970-01-01T00:00:00Z").unwrap(), 0);
        for bad in [
            "2026-11-14",
            "2026-13-01T00:00:00Z",
            "2026-11-14T00:00:00",
            "x",
        ] {
            assert!(parse_origin(bad).is_err(), "{bad}");
        }
    }
}
