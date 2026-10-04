//! `system-compiler`: command line entry point.
//!
//! For now it loads the system data (the bundled `data/system.toml`, or a
//! path given as the only argument), validates it, and prints one line per
//! frame of the registry it would emit. Compilation and the hub connection
//! arrive in later tasks.

use std::process::ExitCode;

use anyhow::Context;
use system_compiler::System;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("system-compiler: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let system = match args.as_slice() {
        [] => System::bundled(),
        [path] => System::load(path).with_context(|| format!("loading {path}"))?,
        _ => anyhow::bail!("usage: system-compiler [path/to/system.toml]"),
    };
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
    Ok(())
}
