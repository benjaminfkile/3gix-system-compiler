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
  src/hub.rs               hub client: config, WebSocket jobs, submit,
                           reconnect, and the serve daemon
  src/main.rs              CLI: compile, describe, keys, frames, serve
  src/rotation.rs          IAU rotational elements to orientation and spin
  src/orbit.rs             osculating elements from a state vector
  src/detmath.rs           deterministic sine and cosine
  tests/data.rs            registry, masses, periods, rotation, opacity
  tests/integration.rs     one year of orbits with gx-core's integrator
  tests/compile.rs         mass conservation, exact empty cells, key listing
  tests/golden.rs          golden SHA-256 hashes and determinism
  tests/golden.json        the golden hashes (see docs/compile.md)
  tests/hub_mock.rs        serve against a mock hub: submissions, 409,
                           400, reconnect, 4401, duplicate jobs
docs/data-sources.md       every source, request, response, and conversion
docs/model.md              how a body becomes matter, and what is not modeled
docs/compile.md            how a chunk key becomes bytes; golden policy
docs/protocol.md           the hub's compiler protocol, from the hub's code
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

## Running against a hub

`serve` holds the hub's compiler WebSocket, compiles every job the hub
dispatches, and submits the sections. The protocol, the status handling, and
the reconnect policy are in `docs/protocol.md`.

```
cargo run --release -p system-compiler -- serve --data data/system.toml
cargo run --release -p system-compiler -- serve --data data/system.toml --workers 8
cargo run --release -p system-compiler -- serve --env-file path/to/.env --once
```

It reads four variables, from the environment or from a `.env` file (the
one named by `--env-file`, or `.env` in the working directory or a parent;
variables already set win). Values are never logged.

| Variable | Also accepted | Meaning |
|---|---|---|
| `GX_HUB_URL` | `HUB_URL` | hub base URL, `http` or `https` |
| `GX_API_KEY` | `COMPILER_API_KEY` | API key with `compile:submit` |
| `GX_COMPILER_ID` | `COMPILER_ID` | the compiler's id |
| `GX_COMPILER_SECRET` | `COMPILER_SECRET` | the compiler's raw shared secret |

The second column holds the names the hub's local seed script writes, so the
`.env` it produces works as is:

1. Start the hub and run its seed script, `scripts/local-seed.ps1` in the
   hub repository. It registers a compiler, creates a build, and writes
   `.env` into a sibling directory named `3gixhub-dummy-compiler` when that
   directory exists. Create the directory first, or copy the values the
   script prints into `.env` here using `.env.example` as the template.
2. Run `serve` with `--env-file` pointing at that file, or copy it here as
   `.env`. Its extra variables are ignored. If it names an `https` hub with
   a self-signed certificate (`INSECURE_TLS=true`), point `GX_HUB_URL` at
   the hub's plain `http` port instead: this compiler always verifies TLS.
3. Request a chunk from the hub; the job, its byte length, the hub's status,
   and the elapsed time appear as one log line on stderr.

`serve` exits non-zero when the hub refuses the credentials (WebSocket close
code `4401`, or `401`/`403` on the upgrade or on a submission), and runs
until interrupted otherwise, reconnecting with backoff when the socket
drops. `--once` exits after the socket closes the first time. `RUST_LOG`
sets the log level (default `info`).

## Specification

- Architecture: `3GIXHub/docs/architecture/space-model.md`
- Matter format v1: `3GIXHub/docs/architecture/matter-format.md`

## Contributing rules

- No infrastructure identifiers, secrets, hostnames, or LAN addresses in code, history, docs, or CI output. This repository is public. Hub URLs, API keys, and compiler secrets come from environment variables documented in `.env.example`.
- No em or en dashes in any text.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` pass on every change.
