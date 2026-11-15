//! Shared helpers for the integration tests.

#![allow(dead_code)]

use gx_core::units::Vec3;
use system_compiler::System;

/// Frame id of the star in `data/system.toml`.
pub const SUN: u64 = 1;
/// Frame id of the third planet.
pub const EARTH: u64 = 4;
/// Frame id of the third planet's satellite.
pub const MOON: u64 = 10;
/// Frame ids of the planets, in order from the star.
pub const PLANETS: [u64; 8] = [2, 3, 4, 5, 6, 7, 8, 9];
/// Frame ids of the gas bodies.
pub const GAS: [u64; 4] = [6, 7, 8, 9];
/// Seconds per day.
pub const DAY: f64 = 86_400.0;
/// 2026-11-14T00:00:00Z, UTC milliseconds.
pub const ORIGIN_UTC_MS: i64 = 1_794_614_400_000;

/// Loads `data/system.toml` from the repository.
pub fn system() -> System {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/system.toml");
    System::load(path).expect("data/system.toml loads")
}

/// A TOML array of three floats as a vector.
pub fn vec3(v: &toml::Value) -> Vec3 {
    let a = v.as_array().expect("an array");
    let f = |i: usize| a[i].as_float().expect("a float");
    Vec3::new(f(0), f(1), f(2))
}
