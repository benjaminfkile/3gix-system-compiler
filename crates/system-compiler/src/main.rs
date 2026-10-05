//! `system-compiler`: command line entry point.
//!
//! Subcommands:
//!
//! - `compile`: compile one chunk key to bytes (file or stdout), reporting
//!   the byte length and SHA-256 on stderr.
//! - `describe`: compile one chunk key, or read bytes from a file with
//!   `--from-file`, decode them with gx-core, and print a JSON summary of
//!   the section or the registry. `--container` decodes the file as a hub
//!   container (`matter-format.md` section 6), the body the hub serves for
//!   a chunk request.
//! - `keys`: list every cell key of a frame at one depth whose section is
//!   not empty, one per line, sorted by `(x, y, z)`.
//! - `frames`: list the frames of the registry with their compiler-side
//!   names, for humans.
//! - `serve`: run as a registered compiler: hold the hub's WebSocket,
//!   compile the jobs it pushes, and submit the sections (see
//!   `docs/protocol.md`). Configuration comes from the environment and an
//!   optional `.env` file.
//!
//! Every subcommand reads the system from `--data`, or from the data baked
//! into the binary when `--data` is omitted.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use gx_core::key::{CellKey, ChunkKey};
use gx_core::matter::{Compression, Section};
use gx_core::registry::ROOT_PARENT;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use system_compiler::compile::{compile, non_empty_keys, CompileOptions};
use system_compiler::hub::{self, Backoff, HubClient, HubConfig, ServeOptions};
use system_compiler::System;

/// Compiles a planetary system into 3GIX matter.
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
        /// Describe the bytes in this file instead of compiling the key.
        #[arg(long)]
        from_file: Option<PathBuf>,
        /// Decode the bytes as a hub container (a section table followed by
        /// the sections of every layer), as the hub serves a chunk.
        #[arg(long, requires = "from_file")]
        container: bool,
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
    /// List the registry's frames with their compiler-side names.
    Frames {
        /// System data file; the bundled data when omitted.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// Connect to the hub and compile the jobs it dispatches until
    /// interrupted. Reads GX_HUB_URL, GX_API_KEY, GX_COMPILER_ID and
    /// GX_COMPILER_SECRET from the environment or a `.env` file.
    Serve {
        /// System data file; the bundled data when omitted.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Jobs compiled and submitted at once.
        #[arg(long, default_value_t = hub::DEFAULT_WORKERS as u16,
              value_parser = clap::value_parser!(u16).range(1..))]
        workers: u16,
        /// Exit after the socket closes the first time, once every received
        /// job has finished.
        #[arg(long)]
        once: bool,
        /// Environment file to load; `.env` in the working directory or a
        /// parent when omitted. Variables already set are not overridden.
        #[arg(long)]
        env_file: Option<PathBuf>,
        /// Seed of the reconnect jitter generator.
        #[arg(long, default_value_t = hub::DEFAULT_BACKOFF_SEED)]
        backoff_seed: u64,
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

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Compile {
            data,
            key,
            out,
            no_compress,
        } => {
            let system = load(data.as_ref())?;
            let key = parse_key(&key)?;
            let opts = CompileOptions {
                compression: if no_compress {
                    Compression::None
                } else {
                    Compression::Zstd
                },
                ..CompileOptions::default()
            };
            let bytes =
                compile(&system, &key, &opts).with_context(|| format!("compiling {key}"))?;
            match out {
                Some(path) => std::fs::write(&path, &bytes)
                    .with_context(|| format!("writing {}", path.display()))?,
                None => {
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
            }
            eprintln!("{} bytes sha256 {}", bytes.len(), sha256_hex(&bytes));
        }
        Command::Describe {
            data,
            key,
            from_file,
            container,
        } => {
            let key = parse_key(&key)?;
            let bytes = match &from_file {
                Some(path) => {
                    std::fs::read(path).with_context(|| format!("reading {}", path.display()))?
                }
                None => {
                    let system = load(data.as_ref())?;
                    compile(&system, &key, &CompileOptions::default())
                        .with_context(|| format!("compiling {key}"))?
                }
            };
            let summary = if container {
                describe_container(&key, &bytes)?
            } else {
                describe(&key, &bytes)?
            };
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Command::Keys { data, frame, depth } => {
            let system = load(data.as_ref())?;
            let mut stdout = std::io::stdout().lock();
            for key in non_empty_keys(&system, frame, depth)? {
                writeln!(stdout, "{key}")?;
            }
        }
        Command::Frames { data } => frames(&load(data.as_ref())?),
        Command::Serve {
            data,
            workers,
            once,
            env_file,
            backoff_seed,
        } => serve(data, usize::from(workers), once, env_file, backoff_seed)?,
    }
    Ok(())
}

/// Runs the `serve` subcommand. Logs go to stderr through `tracing`, at
/// `info` unless `RUST_LOG` says otherwise.
fn serve(
    data: Option<PathBuf>,
    workers: usize,
    once: bool,
    env_file: Option<PathBuf>,
    backoff_seed: u64,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    hub::load_dotenv(env_file.as_deref())?;
    let config = HubConfig::from_env()?;
    let system = Arc::new(load(data.as_ref())?);
    let client = HubClient::new(config)?.with_seed(backoff_seed);
    let opts = ServeOptions {
        workers,
        once,
        reconnect: Backoff::standard(backoff_seed),
        ..ServeOptions::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    let shutdown = async {
        if tokio::signal::ctrl_c().await.is_err() {
            // No signal handler: run until the process is killed.
            std::future::pending::<()>().await;
        }
    };
    runtime
        .block_on(hub::serve(system, client, opts, shutdown))
        .map_err(|e| {
            tracing::error!(error = %e, "compiler stopped on an error");
            anyhow::Error::new(e)
        })?;
    Ok(())
}

/// Lowercase hexadecimal SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Decodes one section or registry with gx-core and summarizes it as JSON.
fn describe(key: &ChunkKey, bytes: &[u8]) -> anyhow::Result<Value> {
    let mut summary = match key {
        ChunkKey::Registry => registry_json(&gx_core::registry::decode(bytes)?),
        ChunkKey::Cell(cell) => section_json(cell, &gx_core::matter::decode(cell, bytes)?),
    };
    summary["key"] = json!(key.to_string());
    summary["bytes"] = json!(bytes.len());
    summary["sha256"] = json!(sha256_hex(bytes));
    Ok(summary)
}

/// Decodes a hub container (`matter-format.md` section 6) with gx-core's
/// container decoder and summarizes it as JSON: one entry per section in
/// layer order. For a cell key, `mass_kg` is the sum over the sections; for
/// the registry, `frame_count` is the total over the registries.
fn describe_container(key: &ChunkKey, bytes: &[u8]) -> anyhow::Result<Value> {
    let mut summary = match key {
        ChunkKey::Registry => {
            let registries = gx_core::container::decode_registry_chunk(bytes)?;
            let frame_count: usize = registries.iter().map(|r| r.frames().len()).sum();
            json!({
                "section_count": registries.len(),
                "frame_count": frame_count,
                "sections": registries.iter().map(registry_json).collect::<Vec<_>>(),
            })
        }
        ChunkKey::Cell(cell) => {
            let sections = gx_core::container::decode_chunk(cell, bytes)?;
            let mass: f64 = sections.iter().map(|s| s.mass().value()).sum();
            json!({
                "section_count": sections.len(),
                "mass_kg": mass,
                "sections": sections.iter().map(|s| section_json(cell, s)).collect::<Vec<_>>(),
            })
        }
    };
    summary["key"] = json!(key.to_string());
    summary["bytes"] = json!(bytes.len());
    summary["sha256"] = json!(sha256_hex(bytes));
    Ok(summary)
}

/// JSON summary of one decoded registry: epoch and every frame.
fn registry_json(reg: &gx_core::registry::Registry) -> Value {
    let frames: Vec<Value> = reg
        .frames()
        .iter()
        .map(|f| {
            let q = f.orientation;
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

/// Prints one line per registry frame, with compiler-side names.
fn frames(system: &System) {
    let registry = system.registry();
    println!(
        "epoch {} s TDB since J2000, {} frames",
        registry.epoch().value(),
        registry.frames().len()
    );
    for f in registry.frames() {
        let name = system
            .body(f.frame_id)
            .map_or(system.root().name.as_str(), |b| b.name.as_str());
        let density = system
            .body(f.frame_id)
            .map_or(0.0, |b| b.mean_density().value());
        println!(
            "{:>3} {:<26} mass {:.6e} kg  extent {:.4e} m  depth {}  density {:.1} kg/m^3",
            f.frame_id,
            name,
            f.mass.value(),
            f.root_extent.value(),
            f.max_depth,
            density
        );
    }
}
