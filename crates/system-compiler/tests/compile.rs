//! The chunk compiler against the bundled data and small hand-made systems:
//! mass conservation for the uniform balls and the polytropes, the
//! polytropes' thin outer layers, the exact empty-cell decision, the key
//! listing, and the error and empty cases.

mod common;

use common::{system, EARTH, GAS, MOON, ORIGIN_UTC_MS, SUN};
use gx_core::key::{CellKey, ChunkKey};
use gx_core::matter::{self, Compression, State};
use gx_core::units::Vec3;
use system_compiler::compile::{intersects, non_empty_keys, section};
use system_compiler::{compile, CompileError, CompileOptions, System};

fn decode(key: &CellKey, bytes: &[u8]) -> matter::Section {
    gx_core::validate::validate(&key.to_string(), bytes).unwrap();
    matter::decode(key, bytes).unwrap()
}

fn compile_cell(s: &System, key: CellKey) -> matter::Section {
    let bytes = compile(s, &ChunkKey::Cell(key), 0, &CompileOptions::default()).unwrap();
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
fn uniform_balls_conserve_mass() {
    let s = system();
    // Achieved: 8.6e-4 (0.086 percent) at depths 1 and 2, the same for
    // every body because each frame extent is 8 mean radii; 5.8e-3 at depth
    // 0, where the ball is 8 samples across.
    for id in [2, EARTH, MOON] {
        for (depth, limit) in [(0, 0.01), (1, 0.005), (2, 0.005)] {
            let err = mass_error(&s, id, depth);
            assert!(err < limit, "frame {id} depth {depth}: {err:e}");
        }
    }
}

#[test]
fn polytropes_conserve_mass_at_the_coarsest_depth() {
    // The coarsest depth that covers a whole frame is 0: one cell. The
    // brief allows 2 percent there; depths 1 and 2 are checked too.
    let s = system();
    for id in std::iter::once(SUN).chain(GAS) {
        for depth in 0..=2 {
            let err = mass_error(&s, id, depth);
            println!("frame {id} depth {depth}: relative mass error {err:.2e}");
            assert!(err < 0.02, "frame {id} depth {depth}: {err:e}");
        }
    }
}

#[test]
fn polytrope_outer_edge_is_thin() {
    // The depth 6 cell holding the surface point (R, 0, 0): cell x index 39
    // spans [0.875 R, R] along x. Every sample whose center is beyond
    // 0.999 R has a density below 1 percent of the mean.
    let s = system();
    for id in std::iter::once(SUN).chain(GAS) {
        let body = s.body(id).unwrap();
        let key = CellKey::new(id, 6, 39, 32, 32).unwrap();
        let sec = compile_cell(&s, key);
        let samples = sec.samples().unwrap();
        let n = usize::from(sec.resolution());
        let edge = sec.edge().value() / n as f64;
        let o = sec.origin();
        let r = body.mean_radius.value();
        let mean = body.mean_density().value();
        let mut checked = 0;
        for (i, x) in samples.iter().enumerate() {
            let (ix, iy, iz) = (i % n, (i / n) % n, i / (n * n));
            let c = Vec3::new(
                o.x + (ix as f64 + 0.5) * edge,
                o.y + (iy as f64 + 0.5) * edge,
                o.z + (iz as f64 + 0.5) * edge,
            );
            if c.length() > 0.999 * r {
                assert!(x.density.value() < 0.01 * mean, "frame {id} sample {i}");
                checked += 1;
            }
        }
        assert!(checked > 0, "frame {id}");
        // Deeper in, the density rises well above the edge.
        assert!(body.density_at_radius(0.5 * r).value() > 0.01 * mean);
    }
}

#[test]
fn polytrope_samples_are_hot_inside() {
    let s = system();
    let key = CellKey::new(SUN, 2, 1, 1, 1).unwrap();
    let sec = compile_cell(&s, key);
    let hottest = sec
        .samples()
        .unwrap()
        .iter()
        .map(|x| x.temperature.value())
        .fold(0.0f64, f64::max);
    // The sample at the center corner averages the core: millions of kelvin.
    assert!(hottest > 1.0e7, "{hottest}");
    let coolest = sec
        .samples()
        .unwrap()
        .iter()
        .filter(|x| x.density.value() > 0.0)
        .map(|x| x.temperature.value())
        .fold(f64::INFINITY, f64::min);
    // The outermost samples average the floor at the effective temperature
    // with hotter matter just inside, weighted by density.
    assert!((5772.0..1.0e5).contains(&coolest), "{coolest}");
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
pole_ra_deg = [0.0, 0.0]
pole_dec_deg = [90.0, 0.0]
prime_meridian_deg = [0.0, 360.0, 0.0]
j2 = 0.0
reference_radius_m = 0.0
principal_moments_kg_m2 = [0.0, 0.0, 0.0]
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
    let mean = body.uniform_sample().density.value() as f32;
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
            0,
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
    let a = compile(&s, &ChunkKey::Cell(key), 0, &raw).unwrap();
    let b = compile(&s, &ChunkKey::Cell(key), 0, &CompileOptions::default()).unwrap();
    assert!(b.len() < a.len());
    assert_eq!(decode(&key, &a), decode(&key, &b));
    assert_eq!(decode(&key, &a), section(&s, &key, &raw).unwrap());
}

#[test]
fn a_bad_resolution_is_reported_not_emitted() {
    let s = system();
    let mut opts = CompileOptions::default();
    opts.resolution_by_depth[1] = 0;
    let r = compile(&s, &"4-1-0-0-0".parse().unwrap(), 0, &opts);
    assert!(matches!(r, Err(CompileError::Invalid(_))));
}

#[test]
fn registry_decodes() {
    let s = system();
    let opts = CompileOptions::default();
    let bytes = compile(&s, &ChunkKey::Registry, ORIGIN_UTC_MS, &opts).unwrap();
    let reg = gx_core::registry::decode(&bytes).unwrap();
    assert_eq!(reg, s.registry_at(ORIGIN_UTC_MS).unwrap());
    assert_eq!(reg.frames().len(), 11);
    // An origin too far from the data epoch is reported, not emitted.
    let far = compile(&s, &ChunkKey::Registry, i64::MAX / 2, &opts);
    assert!(matches!(far, Err(CompileError::Registry(_))));
}

#[test]
fn cells_do_not_depend_on_the_origin() {
    let s = system();
    let key: ChunkKey = "6-2-1-2-1".parse().unwrap();
    let opts = CompileOptions::default();
    assert_eq!(
        compile(&s, &key, 0, &opts).unwrap(),
        compile(&s, &key, ORIGIN_UTC_MS, &opts).unwrap()
    );
}
