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

## Specification

- Architecture: `3GIXHub/docs/architecture/space-model.md`
- Matter format v1: `3GIXHub/docs/architecture/matter-format.md`

## Contributing rules

- No infrastructure identifiers, secrets, hostnames, or LAN addresses in code, history, docs, or CI output. This repository is public. Hub URLs, API keys, and compiler secrets come from environment variables documented in `.env.example`.
- No em or en dashes in any text.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` pass on every change.
