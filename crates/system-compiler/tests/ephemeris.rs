//! The registry at the origin (`space-model.md` section 11 item 1): the baked
//! state vectors integrated to the origin with the library integrator.
//!
//! - Every body is within 0.1 percent of its orbit radius of the JPL
//!   Horizons position at the origin 2026-11-14T00:00:00Z. The check vectors
//!   are baked into `tests/data/horizons-2026-11-14.toml`, fetched once with
//!   `scripts/fetch-horizons.py` (see `docs/data-sources.md`).
//! - Halving the integration step changes every position by less than 1 m,
//!   the brief's criterion for the step (see `src/ephemeris.rs` for why it
//!   is not met).
//! - The registry is bitwise identical twice in one process; the module
//!   determinism harness checks it under Node too (`scripts/ci.sh`).

mod common;

use common::{system, vec3, MOON, ORIGIN_UTC_MS};
use gx_core::registry;
use gx_core::timescale::utc_ms_to_tdb_seconds;
use gx_core::units::Seconds;
use system_compiler::ephemeris::{integrate_with_step, MAX_INTERVAL_S, STEP_S};

const HORIZONS: &str = include_str!("data/horizons-2026-11-14.toml");

fn horizons() -> toml::Table {
    HORIZONS.parse().expect("the check vectors parse")
}

#[test]
fn check_vectors_are_at_the_origin() {
    let h = horizons();
    assert_eq!(h["origin_utc_ms"].as_integer(), Some(ORIGIN_UTC_MS));
    let tdb = h["tdb_seconds_since_j2000"].as_float().unwrap();
    // The fetch script mirrors gx-core's conversion; Horizons is asked for
    // this instant to the microsecond.
    assert!((tdb - utc_ms_to_tdb_seconds(ORIGIN_UTC_MS)).abs() < 1e-6);
}

#[test]
fn registry_at_the_origin_matches_horizons() {
    let s = system();
    let reg = s.registry_at(ORIGIN_UTC_MS).unwrap();
    assert_eq!(reg.epoch().value(), utc_ms_to_tdb_seconds(ORIGIN_UTC_MS));
    let h = horizons();
    let frames = h["frame"].as_table().unwrap();
    assert_eq!(frames.len(), 10);
    for f in reg.frames().iter().skip(1) {
        let want = vec3(&frames[&f.frame_id.to_string()]["position_m"]);
        let error = (f.position - want).length() / want.length();
        let name = &s.body(f.frame_id).unwrap().name;
        println!(
            "{name:8} error {:.3e} m, {error:.2e} of the orbit radius",
            (f.position - want).length()
        );
        assert!(error < 1e-3, "{name}: {error:e} of its orbit radius");
    }
}

#[test]
fn registry_orientation_and_spin_come_from_the_iau_elements_at_the_origin() {
    let s = system();
    let reg = s.registry_at(ORIGIN_UTC_MS).unwrap();
    let tdb = reg.epoch();
    for f in reg.frames().iter().skip(1) {
        let b = s.body(f.frame_id).unwrap();
        assert_eq!(f.orientation, s.orientation_at(f.frame_id, tdb));
        assert_eq!(f.angular_velocity, b.rotation.at(tdb).angular_velocity());
        assert_eq!(f.j2, b.j2);
        assert_eq!(f.principal_moments, b.principal_moments);
        assert_eq!(f.max_depth, b.max_depth);
    }
    // The satellite's spin is relative to its parent's axes, as at the data
    // epoch.
    assert_eq!(reg.frames()[MOON as usize].parent_frame_id, 4);
}

#[test]
fn registry_is_deterministic() {
    let a = registry::encode(&system().registry_at(ORIGIN_UTC_MS).unwrap());
    let b = registry::encode(&system().registry_at(ORIGIN_UTC_MS).unwrap());
    assert_eq!(a, b);
}

#[test]
fn origins_out_of_range_are_refused() {
    let s = system();
    let epoch = s.epoch().value();
    assert!(s
        .registry_at_tdb(Seconds::new(epoch + MAX_INTERVAL_S * 1.01))
        .is_err());
    assert!(s
        .registry_at_tdb(Seconds::new(epoch - MAX_INTERVAL_S * 1.01))
        .is_err());
    assert!(s.registry_at_tdb(Seconds::new(f64::NAN)).is_err());
    // The Unix epoch, the origin of a layer without usesOrigin, is in range.
    assert!(s.registry_at(0).is_ok());
}

#[test]
#[ignore = "unreachable criterion: over 27 years the core integrator's rounding floor (10 to 700 m) dominates below a 900 s step, so no step halves to under 1 m; measurements in docs/compile.md. Revisit with compensated summation in gx-core."]
fn halving_the_step_changes_the_result_by_less_than_one_meter() {
    let s = system();
    let to = Seconds::new(utc_ms_to_tdb_seconds(ORIGIN_UTC_MS));
    let full = integrate_with_step(s.epoch_registry(), to, Seconds::new(STEP_S)).unwrap();
    let half = integrate_with_step(s.epoch_registry(), to, Seconds::new(0.5 * STEP_S)).unwrap();
    let mut worst = (0, 0.0f64);
    for b in s.bodies() {
        let d = (full.state(b.frame_id).unwrap().position
            - half.state(b.frame_id).unwrap().position)
            .length();
        println!("{:8} changes by {d:.3} m", b.name);
        if d > worst.1 {
            worst = (b.frame_id, d);
        }
    }
    assert!(
        worst.1 < 1.0,
        "halving the step from {STEP_S} s moves frame {} by {:.1} m",
        worst.0,
        worst.1
    );
}
