# 3gix-system-compiler

A compiler for the 3GIX space runtime. It compiles a planetary system into matter: a frame registry with real masses, state vectors, and rotation at an epoch, and low-resolution density fields for each body. It is the first real compiler and the hub's end-to-end test.

Compilers are allowed to know what things are. This one knows about a star, eight planets, and one moon. The renderer never learns any of that from what this compiler emits, because the matter format has no place to write it.

## What it does

- Holds a WebSocket connection to the hub's compiler endpoint and receives compilation jobs.
- For the reserved `registry` key, emits the frame registry: one root frame for the system barycenter and one frame per body, with mass, position, velocity, orientation, and angular velocity at the epoch, from a baked ephemeris data file.
- For any other key, emits a matter section for that cell: density, state, temperature, albedo, roughness, and attenuation sampled from a per-body model, or an empty section when the cell holds nothing.
- Validates every section with the core library before submitting it.
- Is deterministic: identical bytes for identical inputs, always.

## What it depends on

- `gx-core`, the shared library: matter format encoder and validator, units, chunk keys, the frame registry.
- The hub's compiler API: `3GIXHub/docs/architecture/compiler-pipeline.md` and `third-party-api.md`.

## Layout

```
Cargo.toml                 virtual workspace
rust-toolchain.toml        pinned Rust toolchain (1.99.0)
data/system.toml           the only place numbers live: masses, radii, state
                           vectors, rotation, and matter parameters, every
                           value sourced in a comment
crates/system-compiler/    library crate `system_compiler` and binary
                           `system-compiler`
  src/model.rs             System: load and validate the data, build the
                           registry, sample the matter model
  src/compile.rs           chunk key to validated, deterministic section
  src/main.rs              CLI: compile, describe, keys, frames
  src/rotation.rs          IAU rotational elements to orientation and spin
  src/orbit.rs             osculating elements from a state vector
  src/detmath.rs           deterministic sine and cosine
  tests/data.rs            registry, masses, periods, rotation, opacity
  tests/integration.rs     one year of orbits with gx-core's integrator
  tests/compile.rs         mass conservation, exact empty cells, key listing
  tests/golden.rs          golden SHA-256 hashes and determinism
  tests/golden.json        the golden hashes (see docs/compile.md)
docs/data-sources.md       every source, request, response, and conversion
docs/model.md              how a body becomes matter, and what is not modeled
docs/compile.md            how a chunk key becomes bytes; golden policy
scripts/ci.sh              format, clippy, tests, dash check
scripts/iau-at-epoch.py    evaluates IAU rotational elements at J2000
```

## Running

```
sh scripts/ci.sh                       # full check

# Compile one chunk key; bytes to a file (or stdout without --out), length
# and SHA-256 to stderr. --no-compress leaves the sample block raw.
cargo run -p system-compiler -- compile --data data/system.toml --key registry --out registry.bin
cargo run -p system-compiler -- compile --data data/system.toml --key 4-2-1-1-1 --out cell.bin

# Decode a compiled key with gx-core and print a JSON summary: geometry,
# resolution, non-vacuum count, and mass, or the registry's frames.
cargo run -p system-compiler -- describe --data data/system.toml --key registry
cargo run -p system-compiler -- describe --data data/system.toml --key 4-2-1-1-1

# Every cell key of a frame at a depth whose section is not empty, sorted.
cargo run -p system-compiler -- keys --data data/system.toml --frame 4 --depth 2

# The registry's frames with their names, for humans.
cargo run -p system-compiler -- frames
```

`--data` is optional everywhere; without it the binary uses the copy of
`data/system.toml` baked in at build time. How a key becomes bytes, the
resolution table, and the golden hash policy are in `docs/compile.md`.

Connecting to the hub arrives in later work; the environment variables in
`.env.example` are for that.

## Specification

- Architecture: `3GIXHub/docs/architecture/space-model.md`
- Matter format v1: `3GIXHub/docs/architecture/matter-format.md`

## Contributing rules

- No infrastructure identifiers, secrets, hostnames, or LAN addresses in code, history, docs, or CI output. This repository is public. Hub URLs, API keys, and compiler secrets come from environment variables documented in `.env.example`.
- No em or en dashes in any text.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` pass on every change.
