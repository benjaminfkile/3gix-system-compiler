# Compiling a chunk

`crates/system-compiler/src/compile.rs` is the pure function at the heart of the module: one chunk key and origin in, one section's bytes out, with no imports and no state. It implements `matter-format.md` sections 2 (chunk keys), 3 (matter sections), 4 (validation), and 5 (the frame registry). The module's `gx_compile` export (`client-compute.md` section 4) and the native CLI both call it.

```rust
pub fn compile(system: &System, key: &ChunkKey, origin_utc_ms: i64, opts: &CompileOptions) -> Result<Vec<u8>, CompileError>
```

## Algorithm

1. Key `registry`: encode `System::registry_at(origin_utc_ms)` with `gx_core::registry::encode`: the baked states integrated to the origin (`docs/model.md`, "The registry at the origin"). The origin is the only input that changes it. Cell sections ignore the origin: the matter never changes.
2. Cell key whose frame id the system does not declare: `CompileError::UnknownFrame`.
3. Cell key of the root frame: an empty section (section 3.4) with the geometry from the root's `root_extent`. The root holds no matter.
4. Cell key of a body frame:
   1. Geometry from the frame's `root_extent` with `CellKey::geometry` (section 3.1).
   2. If the cell's depth is above the frame's `max_depth` in the registry, emit an empty section. Section 5.2 defines `max_depth` as the deepest cell the frame's compilers fill with non-empty matter, so filling deeper cells would contradict the registry this compiler publishes.
   3. If the cell cube does not intersect the ball of mean radius, emit an empty section. The test is exact: per axis the point of the cube nearest the body center is the center clamped to the cube, and the cell intersects when that point's squared distance is at most the squared radius. A cube that touches the sphere at a single point intersects (its section is non-empty, with every sample vacuum); a cube beyond the radius by any margin, even one ulp, is empty.
   4. Otherwise take `n = resolution_by_depth[depth]` and fill the `n^3` grid in index order (x fastest). For a uniform body each sample's density is `fraction * mean_density`, where `fraction` is the sub-sampled share of the sub-cube inside the ball. For a polytrope each sub-sample point inside the ball contributes the profile's `theta^n` and `theta^n T` at `s = d^2 / R^2`; the density is `rho_c` times the mean of `theta^n` over the sub-cube's points, the temperature is the density-weighted mean, and the radiating temperature is the profile's for the sub-cube's radial span (`docs/model.md`, "Radiating temperature"); a uniform body's radiating temperature is its temperature. Every sample's occupancy is `fraction`, for every state alike (see "Occupancy" below). A sample with density above 0 takes the body's state, albedo, roughness, and attenuation; a sample with density 0 is vacuum with every channel 0, occupancy included.
   5. Build the section with `gx_core::matter::Section::new`, so the library's rules apply, then `gx_core::matter::encode` with the chosen compression.
5. Run `gx_core::validate::validate(key, bytes)` on the result and return `CompileError::Invalid` if it fails. A compiler never emits bytes the hub would reject.

`non_empty_keys(system, frame_id, depth)` lists every cell key whose section is non-empty, using the same intersection test on the cells overlapping the ball's bounding box, sorted by `(x, y, z)`.

## Resolution table

| Depth | Samples per axis |
|---|---|
| 0 | 32 |
| 1 | 32 |
| 2 | 16 |
| 3, 4 | 32 |
| 5 | 48 |
| 6 and beyond | 64, the format's maximum |

Every body frame has an extent of 8 mean radii. At depth 2 the cell edge is 2 radii and the eight central cells each hold one octant of the ball, which spans half of each cell along each axis: at 16 samples per axis the diameter is 16 samples (64 sub-sampling points). At depth 1 the cell edge is 4 radii and the ball spans only a quarter of each cell along each axis, so depth 1 takes 32 samples per axis to put the same 16 samples across the diameter (at 16 the summed mass was 0.58 percent high). Depth 0, one cell holding the whole body and the coarsest depth that covers it, takes 32 so the ball is 8 samples across: at 8 per axis the ball was 2 samples across, the uniform balls' summed mass was 4.4 percent high and the polytropes' up to 1.0 percent off; at 32 they are 0.58 percent and 6.1e-7.

## Sub-sampling

The fraction of a sub-cube inside the ball comes from a fixed stratified lattice, never from random points. The cell of edge `e` is divided into `(4n)^3` equal boxes of edge `step = e / (4n)`, and each box contributes its center point:

```
coordinate_k = origin + step * (k + 0.5),   k = 0 .. 4n - 1, per axis
```

Each sample's sub-cube holds `4 x 4 x 4 = 64` of those points, at offsets `(i + 0.5) / 4` of its edge. A point is inside when `x^2 + y^2 + z^2 <= r^2`, summed in that order, the same rule as `Body::contains`. `fraction = inside / 64`.

Mass conservation (`tests/compile.rs`): the summed mass of the non-empty depth 1 cells and of the non-empty depth 2 cells of the uniform frames is within 0.5 percent of each frame's mass in the data file (achieved 8.6e-4). The polytropes' summed mass at depths 0, 1, and 2 is within 2 percent (achieved below 7e-6, because their density falls to 0 smoothly at the surface where the ball's edge cuts the lattice). The sampled densities exist for rendering only; the registry's `mass` stays authoritative (section 5.2).

## Occupancy

`matter-format.md` section 3.3 (format version 3): occupancy is the share of a sample's sub-cube that its matter fills, and the level set `occupancy = 0.5` of solid and fluid samples is the surface. The compiler's finer structure is the stratified lattice above, so a sample's occupancy is `fraction = inside / 64`, the same count that scales a uniform body's density. The boundary is the ball of mean radius for every body: a solid or fluid body's surface, and the outer boundary of a gas or plasma body's gas (whose occupancy is carried but forms no surface). A sample wholly inside the ball has occupancy 1 whatever its density; a polytrope's outermost samples have occupancy near 1 with densities far below the mean. A sample whose points are all outside, or whose density is 0, is vacuum with occupancy 0.

`tests/occupancy.rs` checks where the 0.5 level lands, read the way the rendering contract reads it (`rendering.md` section 2.1: corner values are the mean of the eight sample centers around a corner, linear between corners). For a synthetic ball of radius 2,718,281 m in a frame of 8,000,000 m, along the six axis directions from the center, the crossing lies within one sample spacing of the radius at depths 2, 4, and 6. Measured largest offsets: 469.0 m at depth 2 (spacing 125,000 m, 0.0038 spacings), 469.0 m at depth 4 (spacing 15,625 m, 0.030 spacings), 19.3 m at depth 6 (spacing 1,953.1 m, 0.0099 spacings), the same for a solid and a gas ball. Four lattice points per sample axis quantize the crossing to about an eighth of a spacing at worst.

## Determinism contract

- Every value is `f64`, computed in a fixed order, then rounded once to `f32` by the library when stored.
- Loops run on one thread in index order. No parallel iterators, no `HashMap` iteration, no system time, no randomness.
- No platform math library: the compiler uses addition, multiplication, division, comparison, and `sqrt`, which IEEE 754 defines exactly on every platform. The IAU elements' sines and cosines come from `detmath`; the integrator uses the library's own deterministic functions (core `docs/determinism.md`).
- Compression is zstd at the level fixed by gx-core, with the zstd crate version pinned by gx-core.
- The module is built for `wasm32-unknown-unknown` without relaxed SIMD or threads, and has no imports.

## Conformance policy

`conformance/` holds fifteen request vectors (`NAME.request.bin`, `NAME.tiles.bin`, `NAME.sha256`) and `index.json` saying what each covers: the registry at two origins (2026-11-14T00:00:00Z and 2026-11-15T12:00:00Z), whole and partial cells of the plasma body, solid bodies, and gas bodies at depths 0 to 6, a root cell, an empty cell, and three frame keys at origin 0 as the hub sends them: the record of the orbiting solid (which has moments and a parent that is not the root), the root's record, and the empty record of an undeclared frame. Each hash is a promise about the bytes every client computes and uploads.

- `tests/conformance.rs` regenerates the vectors natively through the module's own entry point and requires every checked-in file to match byte for byte.
- `scripts/ci.sh` builds the WebAssembly module and runs the core library's determinism harness on it: every vector through the native module host (wasmtime) and through Node's WebAssembly, both required to give the checked-in hash.

Any change to `data/system.toml`, the resolution table, the sub-sampling scheme, the polytrope tables, the integration step, the compression, the occupancy rule, or the gx-core version (including a `Cargo.lock` update that moves it) that changes a hash is a conformance change and must be reviewed as one. It is also a new module hash: the layer must be published again (`scripts/publish.sh`). To regenerate:

```
cargo run -p system-compiler --release -- conformance --out conformance
```

Then commit the diff of `conformance/` in the same change as its cause, saying why the bytes changed.
