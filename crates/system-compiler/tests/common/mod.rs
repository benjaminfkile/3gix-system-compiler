//! Shared helpers for the integration tests.

#![allow(dead_code)]

use system_compiler::System;

/// Frame id of the Sun in `data/system.toml`.
pub const SUN: u64 = 1;
/// Frame id of the Earth.
pub const EARTH: u64 = 4;
/// Frame id of the Moon.
pub const MOON: u64 = 10;
/// Frame ids of the planets, in order from the Sun.
pub const PLANETS: [u64; 8] = [2, 3, 4, 5, 6, 7, 8, 9];
/// Seconds per day.
pub const DAY: f64 = 86_400.0;

/// Loads `data/system.toml` from the repository.
pub fn system() -> System {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/system.toml");
    System::load(path).expect("data/system.toml loads")
}
