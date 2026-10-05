# Compiling a chunk

`crates/system-compiler/src/compile.rs` is the pure function at the heart of the compiler: one chunk key in, one section's bytes out, with no network and no state. It implements `matter-format.md` sections 2 (chunk keys), 3 (matter sections), 4 (validation), and 5 (the frame registry).

```rust
pub fn compile(system: &System, key: &ChunkKey, opts: &CompileOptions) -> Result<Vec<u8>, CompileError>
```

## Algorithm

1. Key `registry`: encode `System::registry()` with `gx_core::registry::encode`.
2. Cell key whose frame id the system does not declare: `CompileError::UnknownFrame`.
3. Cell key of the root frame: an empty section (section 3.4) with the geometry from the root's `root_extent`. The root holds no matter.
4. Cell key of a body frame:
   1. Geometry from the frame's `root_extent` with `CellKey::geometry` (section 3.1).
   2. If the cell's depth is above the frame's `max_depth` in the registry, emit an empty section. Section 5.2 defines `max_depth` as the deepest cell the frame's compilers fill with non-empty matter, so filling deeper cells would contradict the registry this compiler publishes.
   3. If the cell cube does not intersect the ball of mean radius, emit an empty section. The test is exact: per axis the point of the cube nearest the body center is the center clamped to the cube, and the cell intersects when that point's squared distance is at most the squared radius. A cube that touches the sphere at a single point intersects (its section is non-empty, with every sample vacuum); a cube beyond the radius by any margin, even one ulp, is empty.
   4. Otherwise take `n = resolution_by_depth[depth]` and fill the `n^3` grid in index order (x fastest). Each sample's density is `fraction * mean_density`, where `fraction` is the sub-sampled share of the sub-cube inside the ball. A sample with `fraction > 0` takes the body's state, temperature, albedo, roughness, and attenuation; a sample with `fraction = 0` is vacuum with every channel 0.
   5. Build the section with `gx_core::matter::Section::new`, so the library's rules apply, then `gx_core::matter::encode` with the chosen compression.
5. Run `gx_core::validate::validate(key, bytes)` on the result and return `CompileError::Invalid` if it fails. A compiler never emits bytes the hub would reject.

`non_empty_keys(system, frame_id, depth)` lists every cell key whose section is non-empty, using the same intersection test on the cells overlapping the ball's bounding box, sorted by `(x, y, z)`.

## Resolution table

| Depth | Samples per axis |
|---|---|
| 0 | 8 |
| 1, 2 | 16 |
| 3, 4 | 32 |
| 5 | 48 |
| 6 and beyond | 64, the format's maximum |

Every body frame has an extent of 8 mean radii, so at depth 2 the eight central cells each hold one octant of the ball at 16 samples per axis, which is 64 samples across the diameter.

## Sub-sampling

The fraction of a sub-cube inside the ball comes from a fixed stratified lattice, never from random points. The cell of edge `e` is divided into `(4n)^3` equal boxes of edge `step = e / (4n)`, and each box contributes its center point:

```
coordinate_k = origin + step * (k + 0.5),   k = 0 .. 4n - 1, per axis
```

Each sample's sub-cube holds `4 x 4 x 4 = 64` of those points, at offsets `(i + 0.5) / 4` of its edge. A point is inside when `x^2 + y^2 + z^2 <= r^2`, summed in that order, the same rule as `Body::contains`. `fraction = inside / 64`.

Mass conservation (`tests/compile.rs`): the summed mass of the non-empty depth 2 cells of frames 1, 4, and 10 is within 0.5 percent of each frame's mass in the data file. Achieved error is 8.6e-4 (0.086 percent) for all three, identical because every frame extent is the same multiple of its radius. The sampled densities exist for rendering only; the registry's `mass` stays authoritative (section 5.2).

## Determinism contract

- Every value is `f64`, computed in a fixed order, then rounded once to `f32` by the library when stored.
- Loops run on one thread in index order. No parallel iterators, no `HashMap` iteration, no system time, no randomness.
- No transcendental functions: the compiler uses only addition, multiplication, division, and comparison, which IEEE 754 defines exactly on every platform. The registry's sines and cosines come from `detmath`.
- Compression is zstd at the level fixed by gx-core, with the zstd crate version pinned by gx-core.
- The tests check that the golden keys compile to identical bytes twice in one process and once in a fresh process through the binary, and that debug and release builds agree (the golden file is produced by the test build and matches the release binary's output).

## Golden policy

`crates/system-compiler/tests/golden.json` stores the SHA-256 of the compiled bytes, with default options, of:

| Key | What it covers |
|---|---|
| `registry` | the frame registry |
| `4-0-0-0-0` | frame 4 at depth 0, the whole body |
| `4-2-1-1-1` | frame 4 at depth 2, a cell holding part of the surface |
| `1-1-0-0-0` | frame 1 at depth 1 |
| `10-3-3-3-3` | frame 10 at depth 3 |
| `6-2-0-0-0` | frame 6 at depth 2, a far corner cell that is empty |
| `0-0-0-0-0` | a root frame cell |

A hash is a promise about what the hub stores. Any change to `data/system.toml`, the resolution table, the sub-sampling scheme, the compression, or the gx-core version (including a `Cargo.lock` update that moves it) that changes a hash is a golden change and must be reviewed as one. To regenerate:

```
UPDATE_GOLDEN=1 cargo test -p system-compiler --test golden
```

Then commit the diff of `golden.json` in the same change as its cause, saying why the bytes changed.
