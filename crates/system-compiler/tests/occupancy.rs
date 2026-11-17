//! Occupancy (`matter-format.md` section 3.3): the surface of a body sits
//! where its boundary is.
//!
//! The surface is the level set `occupancy = 0.5`. These tests read it the
//! way the rendering contract does (`rendering.md` section 2.1): on the
//! corner lattice of the cells, where a corner's value is the mean of the
//! eight sample centers around it, linearly interpolated between corners.
//! The frame center is a corner of every depth's lattice, so the lattice
//! lines through it along the axes are lines through the center. The
//! matter is a synthetic ball, solid and gas.

use std::collections::BTreeMap;

use gx_core::key::{CellKey, ChunkKey};
use gx_core::matter::{self, Section, State};
use system_compiler::{compile, CompileOptions, System};

/// Frame id of the synthetic body.
const BODY: u64 = 1;
/// Edge of the body frame's root cube, meters.
const EXTENT: f64 = 8.0e6;
/// Radius of the synthetic ball, meters: not a multiple of any sample
/// spacing, so the boundary cuts sub-cubes at arbitrary places.
const RADIUS: f64 = 2.718_281e6;

/// A one-body system: a ball of [`RADIUS`] in a frame of [`EXTENT`], solid
/// or gas.
fn ball(state: &str) -> System {
    let (structure, attenuation) = if state == "solid" {
        ("state = \"solid\"", "attenuation_m2_per_kg = 0.0\n")
    } else {
        ("state = \"gas\"\npolytropic_index = 1", "")
    };
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
frame_id = {BODY}
parent_frame_id = 0
name = "a"
mass_kg = 6.0e23
mean_radius_m = {RADIUS:?}
position_m = [1.0e11, 0.0, 0.0]
velocity_m_per_s = [0.0, 3.0e4, 0.0]
pole_ra_deg = [0.0, 0.0]
pole_dec_deg = [90.0, 0.0]
prime_meridian_deg = [0.0, 360.0, 0.0]
j2 = 0.0
reference_radius_m = 0.0
principal_moments_kg_m2 = [0.0, 0.0, 0.0]
{structure}
temperature_k = 250.0
albedo = [0.3, 0.3, 0.3]
roughness = 0.5
{attenuation}root_extent_m = {EXTENT:?}
max_depth = 6
"#
    )
    .parse()
    .unwrap()
}

/// The occupancy field of one frame at one depth, addressed by sample
/// index relative to the frame center: sample `k` along an axis spans
/// `[k h, (k + 1) h]` for the sample spacing `h`. Cells are compiled
/// through the module's `compile` and decoded on first use.
struct Field<'a> {
    system: &'a System,
    depth: u8,
    n: i64,
    half: i64,
    cells: BTreeMap<(i64, i64, i64), Section>,
}

impl<'a> Field<'a> {
    fn new(system: &'a System, depth: u8) -> Self {
        let n = i64::from(CompileOptions::default().resolution_by_depth[usize::from(depth)]);
        Field {
            system,
            depth,
            n,
            half: n * (1 << depth) / 2,
            cells: BTreeMap::new(),
        }
    }

    /// Sample spacing, meters.
    fn spacing(&self) -> f64 {
        EXTENT / (self.n * (1 << self.depth)) as f64
    }

    /// Occupancy of the sample at center-relative index `(x, y, z)`.
    fn occupancy(&mut self, x: i64, y: i64, z: i64) -> f64 {
        let (n, half) = (self.n, self.half);
        let (ax, ay, az) = (x + half, y + half, z + half);
        let cell = (ax / n, ay / n, az / n);
        let (system, depth) = (self.system, self.depth);
        let section = self.cells.entry(cell).or_insert_with(|| {
            let key =
                CellKey::new(BODY, depth, cell.0 as u32, cell.1 as u32, cell.2 as u32).unwrap();
            let bytes =
                compile(system, &ChunkKey::Cell(key), 0, &CompileOptions::default()).unwrap();
            matter::decode(&key, &bytes).unwrap()
        });
        if section.is_empty() {
            return 0.0;
        }
        let s = section.samples().unwrap();
        let i = (ax % n + n * (ay % n + n * (az % n))) as usize;
        s.occupancy(i).value()
    }

    /// The corner value at corner `c` along an axis through the center: the
    /// mean of the eight samples around the lattice point `c h` on that
    /// axis. `axis` is 0, 1, or 2 and `sign` the direction along it.
    fn corner(&mut self, axis: usize, sign: i64, c: i64) -> f64 {
        let mut sum = 0.0;
        // Sample indices on both sides of the point: c - 1 and c along the
        // axis, -1 and 0 across it.
        for along in [c - 1, c] {
            for a in [-1, 0] {
                for b in [-1, 0] {
                    let mut idx = [a, b, 0];
                    idx.rotate_right(2 - axis);
                    idx[axis] = if sign > 0 { along } else { -along - 1 };
                    sum += self.occupancy(idx[0], idx[1], idx[2]);
                }
            }
        }
        sum / 8.0
    }

    /// Distance from the center at which the corner values along one
    /// direction cross 0.5, linearly interpolated between corners. Scans
    /// the corners within three samples of `RADIUS`, which must hold
    /// exactly one crossing.
    fn crossing(&mut self, axis: usize, sign: i64) -> f64 {
        let h = self.spacing();
        let first = (RADIUS / h).floor() as i64 - 3;
        let last = (RADIUS / h).ceil() as i64 + 3;
        let values: Vec<f64> = (first..=last).map(|c| self.corner(axis, sign, c)).collect();
        assert!(values[0] > 0.5 && values[values.len() - 1] < 0.5);
        assert!(
            values.windows(2).all(|w| w[1] <= w[0]),
            "not monotone: {values:?}"
        );
        let i = values.iter().position(|&v| v < 0.5).unwrap();
        let (a, b) = (values[i - 1], values[i]);
        let c0 = (first + i as i64 - 1) as f64;
        (c0 + (a - 0.5) / (a - b)) * h
    }
}

/// The 0.5 level of occupancy crosses every line through the center along
/// the axes, both ways, at `RADIUS` within one sample spacing. Returns the
/// largest offset found, in sample spacings.
fn worst_offset(system: &System, depth: u8) -> (f64, f64) {
    let mut field = Field::new(system, depth);
    let h = field.spacing();
    let mut worst = 0.0f64;
    for axis in 0..3 {
        for sign in [1, -1] {
            let at = field.crossing(axis, sign);
            let offset = (at - RADIUS).abs();
            assert!(
                offset <= h,
                "depth {depth} axis {axis} sign {sign}: crossing at {at} m, \
                 boundary at {RADIUS} m, spacing {h} m"
            );
            worst = worst.max(offset);
        }
    }
    (worst, h)
}

#[test]
fn solid_surface_sits_on_the_boundary_at_three_depths() {
    let system = ball("solid");
    for depth in [2, 4, 6] {
        let (worst, h) = worst_offset(&system, depth);
        println!(
            "solid depth {depth}: spacing {h:.1} m, largest offset {worst:.1} m \
             ({:.4} spacings)",
            worst / h
        );
    }
}

#[test]
fn gas_occupancy_marks_the_outer_boundary_too() {
    let system = ball("gas");
    for depth in [2, 4, 6] {
        let (worst, h) = worst_offset(&system, depth);
        println!(
            "gas depth {depth}: spacing {h:.1} m, largest offset {worst:.1} m \
             ({:.4} spacings)",
            worst / h
        );
    }
}

#[test]
fn occupancy_is_one_inside_zero_in_vacuum_and_shares_the_density_count() {
    let system = ball("solid");
    let body = system.body(BODY).unwrap();
    let mean = body.mean_density().value();
    // Depth 2, cell (2, 1, 1): from the center outwards along x, through
    // the boundary.
    let key = CellKey::new(BODY, 2, 2, 1, 1).unwrap();
    let bytes = compile(&system, &ChunkKey::Cell(key), 0, &CompileOptions::default()).unwrap();
    let section = matter::decode(&key, &bytes).unwrap();
    let samples = section.samples().unwrap();
    let (mut full, mut cut, mut vacuum) = (0, 0, 0);
    for (i, s) in samples.iter().enumerate() {
        let occupancy = samples.occupancy(i).value();
        assert_eq!(s.occupancy.value(), occupancy);
        if s.state == State::Vacuum {
            assert_eq!(occupancy, 0.0);
            vacuum += 1;
            continue;
        }
        // A uniform body's density is its mean density times the share of
        // the sub-cube inside, the same count as occupancy (to f32).
        let expected = (occupancy * mean) as f32;
        assert!(
            (s.density.value() as f32 - expected).abs() <= expected * 1e-6,
            "sample {i}"
        );
        if occupancy == 1.0 {
            full += 1;
        } else {
            assert!(occupancy > 0.0 && occupancy < 1.0);
            cut += 1;
        }
    }
    assert!(full > 0 && cut > 0 && vacuum > 0);

    // A gas body's occupancy is 1 deep inside even where its density is far
    // from the mean.
    let gas = ball("gas");
    let key = CellKey::new(BODY, 2, 1, 1, 1).unwrap();
    let bytes = compile(&gas, &ChunkKey::Cell(key), 0, &CompileOptions::default()).unwrap();
    let section = matter::decode(&key, &bytes).unwrap();
    let samples = section.samples().unwrap();
    let last = samples.len() - 1;
    // The last sample of cell (1, 1, 1) touches the center.
    let center = samples.get(last).unwrap();
    assert_eq!(center.state, State::Gas);
    assert_eq!(center.occupancy.value(), 1.0);
}
