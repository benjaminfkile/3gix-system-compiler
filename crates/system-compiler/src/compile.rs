//! The chunk compiler: one chunk key in, one validated section out.
//!
//! This is the pure function at the heart of the compiler. It implements
//! `matter-format.md` section 2 (chunk keys), section 3 (matter sections,
//! including the empty section of 3.4), section 4 (every output is run
//! through the library validator before it is returned), and section 5 (the
//! frame registry under the reserved key). The algorithm, the resolution
//! table, the sub-sampling scheme, and the determinism contract are written
//! up in `docs/compile.md`.
//!
//! Determinism: every value is computed in `f64` in a fixed order, loops run
//! in index order on one thread, and nothing depends on time, randomness, or
//! hash iteration order. Identical inputs give identical bytes.

use gx_core::error::ValidationError;
use gx_core::key::{CellGeometry, CellKey, ChunkKey, MAX_DEPTH};
use gx_core::matter::{self, Compression, Sample, Samples, Section, MAX_RESOLUTION};
use gx_core::registry;
use gx_core::units::{Density, Meters};

use crate::model::{Body, System};

/// Sub-sampling points per axis inside one sample's sub-cube. The fraction
/// of a sub-cube inside the body is the share of its
/// `SUBSAMPLES^3` stratified points that fall inside the ball.
pub const SUBSAMPLES: u32 = 4;

/// The default resolution table: 8 at depth 0, 16 at depths 1 and 2, 32 at
/// depths 3 and 4, 48 at depth 5, and 64 (the format's maximum) from depth
/// 6 on.
pub const DEFAULT_RESOLUTION_BY_DEPTH: [u8; 32] = default_resolution_table();

const fn default_resolution_table() -> [u8; 32] {
    let top = if 64 < MAX_RESOLUTION {
        64
    } else {
        MAX_RESOLUTION
    };
    let mut t = [top; 32];
    t[0] = 8;
    t[1] = 16;
    t[2] = 16;
    t[3] = 32;
    t[4] = 32;
    t[5] = 48;
    t
}

/// How sections are compiled.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CompileOptions {
    /// Samples per axis of a non-empty section, indexed by cell depth.
    pub resolution_by_depth: [u8; 32],
    /// Compression of the sample block.
    pub compression: Compression,
}

impl Default for CompileOptions {
    /// [`DEFAULT_RESOLUTION_BY_DEPTH`] and [`Compression::Zstd`].
    fn default() -> Self {
        CompileOptions {
            resolution_by_depth: DEFAULT_RESOLUTION_BY_DEPTH,
            compression: Compression::Zstd,
        }
    }
}

/// Why a chunk key could not be compiled.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// The cell key names a frame the system does not declare.
    #[error("frame_id {0} is not declared by this system")]
    UnknownFrame(u64),
    /// The section could not be built, or the encoded bytes fail the
    /// library validator. A compiler never emits bytes the hub would reject.
    #[error("compiled section is invalid: {0}")]
    Invalid(#[from] ValidationError),
}

/// Compiles one chunk key to the bytes of its section.
///
/// - `registry`: the system's frame registry (`matter-format.md` section 5).
/// - A cell of an undeclared frame: [`CompileError::UnknownFrame`].
/// - A cell of the root frame: an empty section; the root holds no matter.
/// - A cell of a body frame: an empty section if the cell cube does not
///   intersect the body's ball of mean radius or the cell is deeper than
///   the frame's declared `max_depth`; otherwise the sampled grid at
///   `opts.resolution_by_depth[depth]` (see [`sample_cell`]).
///
/// The bytes are checked with `gx_core::validate::validate` under the key's
/// canonical string before they are returned.
pub fn compile(
    system: &System,
    key: &ChunkKey,
    opts: &CompileOptions,
) -> Result<Vec<u8>, CompileError> {
    let bytes = match key {
        ChunkKey::Registry => registry::encode(&system.registry()),
        ChunkKey::Cell(cell) => matter::encode(&section(system, cell, opts)?, opts.compression),
    };
    gx_core::validate::validate(&key.to_string(), &bytes)?;
    Ok(bytes)
}

/// Builds the section for one cell key, before encoding. See [`compile`].
pub fn section(
    system: &System,
    key: &CellKey,
    opts: &CompileOptions,
) -> Result<Section, CompileError> {
    let root = system.root();
    if key.frame_id == root.frame_id {
        let g = key.geometry(root.root_extent);
        return Ok(Section::empty(*key, g.origin, g.edge)?);
    }
    let body = system
        .body(key.frame_id)
        .ok_or(CompileError::UnknownFrame(key.frame_id))?;
    let g = key.geometry(body.root_extent);
    if key.depth > body.max_depth || !intersects(body, &g) {
        return Ok(Section::empty(*key, g.origin, g.edge)?);
    }
    let n = opts.resolution_by_depth[usize::from(key.depth.min(MAX_DEPTH))];
    let samples = sample_cell(body, &g, n);
    Ok(Section::new(*key, g.origin, g.edge, n, samples)?)
}

/// Returns `true` if the cell cube touches or overlaps the body's ball of
/// mean radius.
///
/// Exact closest-point test: per axis the point of the cube nearest the
/// body center (the frame origin) is the center coordinate clamped to the
/// cube, and the cell intersects when that point's squared distance is at
/// most the squared radius. A cube touching the sphere at one point
/// intersects; a cube beyond the radius by any margin does not.
pub fn intersects(body: &Body, g: &CellGeometry) -> bool {
    let e = g.edge.value();
    let nearest = |lo: f64| -> f64 {
        let hi = lo + e;
        if hi < 0.0 {
            hi
        } else if lo > 0.0 {
            lo
        } else {
            0.0
        }
    };
    let (x, y, z) = (
        nearest(g.origin.x),
        nearest(g.origin.y),
        nearest(g.origin.z),
    );
    let r = body.mean_radius.value();
    x * x + y * y + z * z <= r * r
}

/// Samples the body's matter on the `n^3` grid of a cell.
///
/// The fraction of each sub-cube inside the ball is estimated from a fixed
/// stratified lattice: the cell is divided into `(SUBSAMPLES * n)^3` equal
/// boxes and each box contributes its center point, so every sub-cube holds
/// `SUBSAMPLES^3` points at the offsets `(i + 0.5) / SUBSAMPLES` of its
/// edge. A point at distance `d` from the center is inside when
/// `d^2 <= r^2`, the same rule as [`Body::contains`]. Density is
/// `fraction * mean_density`; the other channels are the body's for a
/// sample with any matter and zero for vacuum.
pub fn sample_cell(body: &Body, g: &CellGeometry, n: u8) -> Samples {
    let m = SUBSAMPLES as usize * usize::from(n);
    let step = g.edge.value() / m as f64;
    // Squared lattice coordinates along each axis, computed once per cell.
    let squares = |origin: f64| -> Vec<f64> {
        (0..m)
            .map(|k| {
                let c = origin + step * (k as f64 + 0.5);
                c * c
            })
            .collect()
    };
    let (sx, sy, sz) = (
        squares(g.origin.x),
        squares(g.origin.y),
        squares(g.origin.z),
    );
    let r = body.mean_radius.value();
    let r2 = r * r;
    let s = SUBSAMPLES as usize;
    let total = f64::from(SUBSAMPLES * SUBSAMPLES * SUBSAMPLES);
    let full = body.sample();
    let mean = full.density.value();
    Samples::from_fn(n, |x, y, z| {
        let (x0, y0, z0) = (x as usize * s, y as usize * s, z as usize * s);
        let mut inside = 0u32;
        for &qz in &sz[z0..z0 + s] {
            for &qy in &sy[y0..y0 + s] {
                for &qx in &sx[x0..x0 + s] {
                    if qx + qy + qz <= r2 {
                        inside += 1;
                    }
                }
            }
        }
        if inside == 0 {
            Sample::VACUUM
        } else {
            Sample {
                density: Density::new(f64::from(inside) / total * mean),
                ..full
            }
        }
    })
}

/// Every cell key of a frame at one depth whose section is not empty,
/// sorted by `(x, y, z)` ascending.
///
/// Empty for the root frame and for depths beyond the frame's `max_depth`.
/// Only the cells overlapping the ball's bounding box are tested, with the
/// same [`intersects`] rule [`compile`] uses, so the list matches the
/// compiler exactly.
pub fn non_empty_keys(
    system: &System,
    frame_id: u64,
    depth: u8,
) -> Result<Vec<CellKey>, CompileError> {
    if frame_id == system.root().frame_id {
        return Ok(Vec::new());
    }
    let body = system
        .body(frame_id)
        .ok_or(CompileError::UnknownFrame(frame_id))?;
    if depth > body.max_depth || depth > MAX_DEPTH {
        return Ok(Vec::new());
    }
    let (lo, hi) = candidate_range(body, depth);
    let mut keys = Vec::new();
    for x in lo..=hi {
        for y in lo..=hi {
            for z in lo..=hi {
                let key = CellKey {
                    frame_id,
                    depth,
                    x,
                    y,
                    z,
                };
                if intersects(body, &key.geometry(body.root_extent)) {
                    keys.push(key);
                }
            }
        }
    }
    Ok(keys)
}

/// Index range per axis covering the ball's bounding box `[-r, r]`, widened
/// by one cell on each side so rounding never drops a candidate.
fn candidate_range(body: &Body, depth: u8) -> (u32, u32) {
    let cells = 1u64 << depth;
    let extent: Meters = body.root_extent;
    let edge = extent.value() / cells as f64;
    let r = body.mean_radius.value();
    let half = extent.value() / 2.0;
    let index = |p: f64| -> i64 { ((p + half) / edge).floor() as i64 };
    let max = cells as i64 - 1;
    let lo = (index(-r) - 1).clamp(0, max);
    let hi = (index(r) + 1).clamp(0, max);
    (lo as u32, hi as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table() {
        let t = DEFAULT_RESOLUTION_BY_DEPTH;
        assert_eq!(&t[..7], &[8, 16, 16, 32, 32, 48, 64]);
        assert!(t[7..].iter().all(|&n| n == 64));
    }
}
