# 3gix-system-compiler

The system module for the 3GIX space runtime. It compiles a planetary system into matter: a frame registry at the universe's origin, with real masses, state vectors, oblateness, moments of inertia, and rotation, and coarse density fields for every body. It implements `space-model.md` section 11 item 1 and the module ABI of `client-compute.md` section 4.

It is a module, not a daemon: a WebAssembly binary with no imports that the client's worker runs (natively or in the browser) and whose output it uploads to the hub. Nothing here talks to a hub except `scripts/publish.sh`, which uploads the module and sets up its layer.

Compilers are allowed to know what things are. This one knows about a star, eight planets, and one moon. The renderer never learns any of that from what this module emits, because the matter format has no place to write it.

## What it does

One request in (`client-compute.md` section 4: chunk key, origin in UTC milliseconds, span), one section out:

- **`registry`**: the frame registry at the origin. The origin is converted to TDB with the core `timescale`; the state vectors baked at J2000 in `data/system.toml` are integrated to it with the core relativistic integrator (first post-Newtonian gravity, oblateness, torques) at a fixed 1800 s step; orientation and spin are the IAU rotational elements evaluated at the origin. The registry's epoch is the origin in TDB. One root frame for the system barycenter and one frame per body.
- **A cell key**: the matter section of that cell. Solid bodies are uniform balls. Gas and plasma bodies are hydrostatic polytropes matching mass and mean radius (index 1 for the gas bodies, index 3 for the plasma body), with temperatures from the polytropic relation. Cells outside a body, deeper than the frame's `max_depth`, or in the root frame are empty sections.
- **Occupancy** (`matter-format.md` section 3.3, format version 3): every sample carries the share of its sub-cube inside the body's outer boundary, the ball of mean radius. It is counted from the same stratified points (4 per axis, 64 per sample) that give the mean density: occupancy is the fraction of them inside. Gas and plasma bodies get it the same way, as the share of the sub-cube inside the outer boundary of the gas. A sample wholly inside has occupancy 1 whatever its density; a vacuum sample has 0. The level set `occupancy = 0.5` of the solid and fluid samples is therefore where the body's boundary is: `tests/occupancy.rs` measures it within a hundredth to three hundredths of a sample spacing of the radius at depths 2, 4, and 6 (`docs/compile.md`, "Occupancy").
- It reads no source tiles (`gx_sources` returns none) and ignores the span.
- Every section is validated with the core library before it is returned.
- Deterministic: identical bytes for identical requests, natively and under Node, on every machine. Pinned by the conformance vectors in `conformance/`.

How a body becomes a frame and matter, the integration step, and what is not modeled: `docs/model.md`. How a key becomes bytes and the conformance policy: `docs/compile.md`. Where every number comes from: `docs/data-sources.md`.

## Layer config

The module's output depends on the origin (the registry does), so its layer runs with `usesOrigin` true. Its matter never changes, so `spanSeconds` is 0. It is attached to every frame. The layer config (`client-compute.md` section 2.2) is:

```json
{
  "module": "<sha256 of target/module/system.wasm>",
  "sourceManifest": null,
  "attachment": "all",
  "exclusive": false,
  "maxDepth": { "0": 0, "1": 6, "2": 6, "3": 6, "4": 6, "5": 6, "6": 6, "7": 6, "8": 6, "9": 6, "10": 6 },
  "spanSeconds": 0,
  "usesOrigin": true
}
```

The `maxDepth` table is the registry's `max_depth` of every frame, the deepest cell this module fills; `cargo run -p system-compiler -- max-depth` prints it. `scripts/publish.sh` builds the module and puts exactly this config as layer `system`.

## What it depends on

- `gx-core`, the core library: matter format encoder and validator, chunk keys, the frame registry, the timescale, the integrator.
- `gx-module-guest`, the core module kit: `gx_module!` turns the `Module` implementation into the module ABI exports.
- The specification: `3GIXHub/docs/architecture/space-model.md`, `client-compute.md`, and `matter-format.md`.

## Layout

```
Cargo.toml                 virtual workspace
rust-toolchain.toml        pinned Rust toolchain (1.99.0) with wasm32-unknown-unknown
data/system.toml           the only place numbers live: masses, radii, state
                           vectors at J2000, rotational elements, J2,
                           moments, and matter parameters, every value
                           sourced in a comment
conformance/               12 request vectors with expected SHA-256
crates/system-compiler/    library crate `system_compiler` (cdylib: the
                           module; rlib: the CLI and tests) and binary
                           `system-compiler`
  build.rs                 linker arguments the module host requires
  src/lib.rs               SystemModule and gx_module!
  src/compile.rs           chunk key and origin to validated section
  src/model.rs             System: load and validate the data, the registry
                           at an origin, the per-body matter model
  src/ephemeris.rs         J2000 states integrated to the origin
  src/polytrope.rs         Lane-Emden solutions and polytrope profiles
  src/rotation.rs          IAU rotational elements at a time
  src/orbit.rs             osculating elements from a state vector
  src/detmath.rs           deterministic sine and cosine
  src/conformance.rs       the conformance vectors (native only)
  src/main.rs              CLI: compile, describe, keys, frames, max-depth,
                           conformance
  tests/ephemeris.rs       registry against Horizons at the origin, step
                           halving, determinism
  tests/data/              Horizons check vectors for 2026-11-14
  tests/data.rs            registry, masses, periods, rotation, J2, moments,
                           opacity
  tests/compile.rs         mass conservation, polytrope edges, exact empty
                           cells, key listing
  tests/conformance.rs     checked-in vectors match the module's output
  tests/occupancy.rs       the occupancy 0.5 level sits on the boundary
  tests/integration.rs     one year of orbits with the core integrator
  tests/describe.rs        the CLI against the library
docs/model.md              how a body becomes frames and matter
docs/compile.md            how a key becomes bytes; conformance policy
docs/data-sources.md       every source, request, response, and conversion
scripts/ci.sh              format, clippy, module build, determinism
                           harness, tests, dash check
scripts/build-module.sh    builds target/module/system.wasm
scripts/publish.sh         uploads the module and puts layer `system`
scripts/fetch-horizons.py  fetches the Horizons check vectors (one-off)
scripts/iau-elements.py    prints the IAU elements from the NAIF kernel
                           (one-off)
```

## Building the module

```
sh scripts/build-module.sh
```

builds the library for `wasm32-unknown-unknown` in release and copies it to `target/module/system.wasm`, printing its size and SHA-256 (its module hash). The module has no imports, uses no relaxed SIMD and no threads, and runs under the core module host and under Node.

## Checking

```
sh scripts/ci.sh
```

runs `cargo fmt --check`, `cargo clippy -D warnings`, the module build, the core determinism harness (every vector in `conformance/` through the native module host and through Node, both required to give the checked-in hash), `cargo test`, and the dash check. The harness comes from the core library checkout cargo fetched for the `gx-core` dependency, or from `GX_CORE_DIR` when set. Node must be on the path.

Known failure: `tests/ephemeris.rs` `halving_the_step_changes_the_result_by_less_than_one_meter` fails. Halving the 1800 s step moves the Moon by 304 m; no step brings that under 1 m, because floating point rounding in the core integrator over 27 years is tens to hundreds of meters and grows as the step shrinks. `docs/model.md`, "Integration step", has the measurements.

## Local compile and describe

The native CLI calls the same `compile` function the module runs.

```
# Compile one chunk key at an origin; bytes to a file (or stdout without
# --out), length and SHA-256 to stderr. --origin takes UTC milliseconds or
# YYYY-MM-DDTHH:MM:SS[.mmm]Z and defaults to the data epoch.
cargo run --release -p system-compiler -- compile --key registry --origin 2026-11-14T00:00:00Z --out registry.bin
cargo run --release -p system-compiler -- compile --key 6-2-1-1-1 --out cell.bin

# Decode a compiled key with gx-core and print a JSON summary: geometry,
# resolution, non-vacuum count, and mass, or the registry's frames.
cargo run --release -p system-compiler -- describe --key registry --origin 2026-11-14T00:00:00Z
cargo run --release -p system-compiler -- describe --key 4-2-1-1-1

# Describe bytes from a file instead of compiling.
cargo run --release -p system-compiler -- describe --key 6-2-1-1-1 --from-file cell.bin

# Every cell key of a frame at a depth whose section is not empty, sorted.
cargo run --release -p system-compiler -- keys --frame 4 --depth 2

# The registry's frames at an origin with their names, for humans.
cargo run --release -p system-compiler -- frames --origin 2026-11-14T00:00:00Z

# The layer's maxDepth table.
cargo run --release -p system-compiler -- max-depth

# Regenerate the conformance vectors after an intended change.
cargo run --release -p system-compiler -- conformance --out conformance
```

`--data` is optional on every command except `conformance`; without it the binary uses the copy of `data/system.toml` baked in at build time, which is what the module bakes too.

## Publishing

```
GX_HUB_URL=... GX_API_KEY=... GX_SPACE_ID=... sh scripts/publish.sh
```

builds the module, uploads it (`PUT /module`, checking the returned hash), and puts layer `system` of the space (`PUT /space/{spaceId}/layer/system`) with the config above. The API key needs the admin capability. The three variables come from the environment only; `.env.example` lists them. Publishing a module with different bytes gives a new module hash and a new build of the space.

## Contributing rules

- No infrastructure identifiers, secrets, hostnames, or LAN addresses in code, history, docs, or CI output. This repository is public. Hub URLs, API keys, and space ids come from environment variables.
- No em or en dashes in any text.
- `sh scripts/ci.sh` passes on every change, apart from the known failure above.
- A change that moves a conformance hash is reviewed as one (`docs/compile.md`).
