//! The chunk compiler against the bundled data and small hand-made systems:
//! mass conservation, the exact empty-cell decision, the key listing, and
//! the error and empty cases.

mod common;

use common::{system, EARTH, MOON, SUN};
use gx_core::key::{CellKey, ChunkKey};
use gx_core::matter::{self, Compression, State};
use system_compiler::compile::{intersects, non_empty_keys, section};
use system_compiler::{compile, CompileError, CompileOptions, System};

fn decode(key: &CellKey, bytes: &[u8]) -> matter::Section {
    gx_core::validate::validate(&key.to_string(), bytes).unwrap();
    matter::decode(key, bytes).unwrap()
}

fn compile_cell(s: &System, key: CellKey) -> matter::Section {
    let bytes = compile(s, &ChunkKey::Cell(key), &CompileOptions::default()).unwrap();
    decode(&key, &bytes)
}

/// Relative error of the summed mass of every non-empty cell of a frame at
/// a depth, against the frame's mass in the data file.
fn mass_error(s: &System, frame_id: u64, depth: u8) -> f64 {
    let keys = non_empty_keys(s, frame_id, depth).unwrap();
    assert!(!keys.is_empty());
    let total: f64 = keys
        .iter()
        .map(|&k| compile_cell(s, k).mass().value())
        .sum();
    let mass = s.body(frame_id).unwrap().mass.value();
    (total - mass).abs() / mass
}

#[test]
fn depth_two_conserves_mass() {
    let s = system();
    let err = mass_error(&s, EARTH, 2);
    // Achieved: 8.6e-4 (0.086 percent) with 4 x 4 x 4 sub-sampling, the
    // same for every body because each frame extent is 8 mean radii.
    assert!(err < 0.005, "relative mass error {err:e}");
    for id in [SUN, MOON] {
        let err = mass_error(&s, id, 2);
        assert!(err < 0.005, "frame {id}: relative mass error {err:e}");
    }
}

#[test]
fn every_depth_two_cell_validates_and_empties_agree_with_keys() {
    let s = system();
    let listed = non_empty_keys(&s, EARTH, 2).unwrap();
    for x in 0..4 {
        for y in 0..4 {
            for z in 0..4 {
                let key = CellKey::new(EARTH, 2, x, y, z).unwrap();
                let sec = compile_cell(&s, key);
                assert_eq!(!sec.is_empty(), listed.contains(&key), "{key}");
            }
        }
    }
}

#[test]
fn keys_at_depth_two_are_the_eight_central_cells() {
    let s = system();
    let keys = non_empty_keys(&s, EARTH, 2).unwrap();
    let strings: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    assert_eq!(
        strings,
        [
            "4-2-1-1-1",
            "4-2-1-1-2",
            "4-2-1-2-1",
            "4-2-1-2-2",
            "4-2-2-1-1",
            "4-2-2-1-2",
            "4-2-2-2-1",
            "4-2-2-2-2"
        ]
    );
    let body = s.body(EARTH).unwrap();
    assert!(keys
        .iter()
        .all(|k| intersects(body, &k.geometry(body.root_extent))));
}

#[test]
fn keys_match_a_full_scan_at_depth_four() {
    let s = system();
    let body = s.body(MOON).unwrap();
    let listed = non_empty_keys(&s, MOON, 4).unwrap();
    let mut scanned = Vec::new();
    for x in 0..16 {
        for y in 0..16 {
            for z in 0..16 {
                let key = CellKey::new(MOON, 4, x, y, z).unwrap();
                if intersects(body, &key.geometry(body.root_extent)) {
                    scanned.push(key);
                }
            }
        }
    }
    assert_eq!(listed, scanned);
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(listed, sorted);
}

/// A one-body system with radius `r` and a frame extent of 8 m: depth 2
/// cells have an edge of 2 m, and cell (0, 1, 1) spans x in [-4, -2] with
/// its nearest point to the center at (-2, 0, 0).
fn tiny(r: f64) -> System {
    format!(
        r#"
[epoch]
tdb_seconds_since_j2000 = 0.0

[root]
frame_id = 0
name = "origin"
mass_kg = 0.0
root_extent_m = 1.0e12
max_depth = 0

[[body]]
frame_id = 1
parent_frame_id = 0
name = "a"
mass_kg = 1.0e3
mean_radius_m = {r:?}
position_m = [1.0e11, 0.0, 0.0]
velocity_m_per_s = [0.0, 3.0e4, 0.0]
pole_ra_deg = 0.0
pole_dec_deg = 90.0
prime_meridian_deg = 0.0
rotation_rate_deg_per_day = 360.0
state = "solid"
temperature_k = 250.0
albedo = [0.3, 0.3, 0.3]
roughness = 0.5
attenuation_m2_per_kg = 0.0
root_extent_m = 8.0
max_depth = 6
"#
    )
    .parse()
    .unwrap()
}

#[test]
fn empty_cell_decision_is_exact() {
    let key = CellKey::new(1, 2, 0, 1, 1).unwrap();
    // Touching at exactly one face point: not empty, though no sample
    // point is inside, so every sample is vacuum.
    let touching = compile_cell(&tiny(2.0), key);
    assert!(!touching.is_empty());
    assert_eq!(touching.mass().value(), 0.0);
    // One ulp short of touching: empty.
    let short = f64::from_bits(2.0f64.to_bits() - 1);
    assert!(compile_cell(&tiny(short), key).is_empty());
    // Overlapping: not empty and holds matter.
    let over = compile_cell(&tiny(2.5), key);
    assert!(!over.is_empty() && over.mass().value() > 0.0);
    // The key listing makes the same decision.
    assert!(non_empty_keys(&tiny(2.0), 1, 2).unwrap().contains(&key));
    assert!(!non_empty_keys(&tiny(short), 1, 2).unwrap().contains(&key));
}

#[test]
fn samples_follow_the_vacuum_rules_and_body_matter() {
    let s = system();
    let key = CellKey::new(EARTH, 2, 1, 1, 1).unwrap();
    let sec = compile_cell(&s, key);
    let body = s.body(EARTH).unwrap();
    let mean = body.mean_density().value() as f32;
    let samples = sec.samples().unwrap();
    assert_eq!(sec.resolution(), 16);
    let (mut full, mut partial, mut vacuum) = (0, 0, 0);
    for x in samples.iter() {
        let d = x.density.value() as f32;
        if d == 0.0 {
            vacuum += 1;
            assert_eq!(x.state, State::Vacuum);
        } else {
            assert_eq!(x.state, body.matter.state);
            assert_eq!(
                x.temperature.value() as f32,
                body.matter.temperature.value() as f32
            );
            if d == mean {
                full += 1;
            } else {
                partial += 1;
                assert!(d < mean);
            }
        }
    }
    assert!(full > 0 && partial > 0 && vacuum > 0);
}

#[test]
fn root_and_unknown_frames_and_deep_cells() {
    let s = system();
    assert!(compile_cell(&s, CellKey::new(0, 3, 4, 4, 4).unwrap()).is_empty());
    assert!(matches!(
        compile(
            &s,
            &"99-0-0-0-0".parse().unwrap(),
            &CompileOptions::default()
        ),
        Err(CompileError::UnknownFrame(99))
    ));
    assert!(matches!(
        non_empty_keys(&s, 99, 0),
        Err(CompileError::UnknownFrame(99))
    ));
    assert!(non_empty_keys(&s, 0, 3).unwrap().is_empty());
    // Frame 4 declares max_depth 6: a cell at the center at depth 7 is empty.
    let deep = CellKey::new(EARTH, 7, 63, 63, 63).unwrap();
    assert!(compile_cell(&s, deep).is_empty());
    assert!(non_empty_keys(&s, EARTH, 7).unwrap().is_empty());
}

#[test]
fn compression_does_not_change_the_samples() {
    let s = system();
    let key = CellKey::new(MOON, 3, 3, 3, 3).unwrap();
    let raw = CompileOptions {
        compression: Compression::None,
        ..CompileOptions::default()
    };
    let a = compile(&s, &ChunkKey::Cell(key), &raw).unwrap();
    let b = compile(&s, &ChunkKey::Cell(key), &CompileOptions::default()).unwrap();
    assert!(b.len() < a.len());
    assert_eq!(decode(&key, &a), decode(&key, &b));
    assert_eq!(decode(&key, &a), section(&s, &key, &raw).unwrap());
}

#[test]
fn a_bad_resolution_is_reported_not_emitted() {
    let s = system();
    let mut opts = CompileOptions::default();
    opts.resolution_by_depth[1] = 0;
    let r = compile(&s, &"4-1-0-0-0".parse().unwrap(), &opts);
    assert!(matches!(r, Err(CompileError::Invalid(_))));
}

#[test]
fn registry_decodes() {
    let s = system();
    let bytes = compile(&s, &ChunkKey::Registry, &CompileOptions::default()).unwrap();
    let reg = gx_core::registry::decode(&bytes).unwrap();
    assert_eq!(reg, s.registry());
    assert_eq!(reg.frames().len(), 11);
}
