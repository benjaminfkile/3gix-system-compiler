//! The system module: compiles a planetary system into 3GIX matter.
//!
//! This crate is a compiler module in the sense of `space-model.md` section
//! 2: a WebAssembly binary with no imports, a pure function from a request
//! to one section, built with `gx-module-guest` to the module ABI v1 of
//! `client-compute.md` section 4. It implements `space-model.md` section 11
//! item 1. It knows what the bodies of the system are; what it emits does
//! not.
//!
//! - [`SystemModule`]: the module. For the `registry` key it returns the
//!   frame registry at the request's origin; for a cell key, the matter
//!   section of that cell; for a frame key `frame-{id}`, the frame record
//!   (`matter-format.md` section 11, `client-compute.md` section 10.2), or
//!   the empty record for a frame it does not declare. It stays ABI
//!   version 1. It reads no source tiles. Built for
//!   `wasm32-unknown-unknown`, [`gx_module_guest::gx_module!`] turns it into
//!   the `gx_sources` and `gx_compile` exports.
//! - [`compile`](mod@compile): one chunk key in, one validated,
//!   deterministic section out (`matter-format.md` sections 2 to 5,
//!   documented in `docs/compile.md`).
//! - [`model`]: the parsed and validated contents of `data/system.toml`, the
//!   frame registry at any origin (`matter-format.md` section 5), and the
//!   per-body matter model (`matter-format.md` section 3).
//! - [`ephemeris`]: the baked state vectors integrated from the data epoch
//!   to the origin with the library integrator.
//! - [`polytrope`]: Lane-Emden solutions and the density and temperature
//!   profiles of the gas and plasma bodies.
//! - [`rotation`]: IAU rotational elements evaluated at a time, converted to
//!   the registry's orientation quaternion and angular velocity.
//! - [`orbit`]: two-body osculating elements from a state vector, used to
//!   check the ephemeris against published periods.
//! - `conformance` (native only): the module's conformance vectors.
//! - [`detmath`]: deterministic sine and cosine in degrees, so that registry
//!   bytes do not depend on the platform math library.
//!
//! The native CLI (`src/main.rs`) calls the same [`compile()`] function.

#![warn(missing_docs)]

pub mod compile;
#[cfg(not(target_arch = "wasm32"))]
pub mod conformance;
pub mod detmath;
pub mod ephemeris;
pub mod model;
pub mod orbit;
pub mod polytrope;
pub mod rotation;

use gx_module_guest::{gx_module, Module, Request, Tiles};

pub use compile::{compile, CompileError, CompileOptions};
pub use model::{Body, Matter, ModelError, RootFrame, Structure, System};

/// The bundled system data file, `data/system.toml`, baked into the module
/// so that output never depends on anything but the request.
pub const SYSTEM_TOML: &str = include_str!("../../../data/system.toml");

/// The system module (`client-compute.md` section 4).
///
/// Its layer runs with `usesOrigin` true: the registry depends on the
/// origin. Cell sections do not, but they receive it like every request.
pub struct SystemModule;

impl Module for SystemModule {
    /// The system module reads no source tiles.
    fn sources(_req: &Request) -> Vec<u64> {
        Vec::new()
    }

    /// Compiles the request's chunk key at its origin with the bundled data
    /// and the default options. The span is ignored: the layer's span length
    /// is 0.
    fn compile(req: &Request, _tiles: &Tiles) -> Result<Vec<u8>, String> {
        let system = System::bundled();
        compile::compile(
            &system,
            &req.key,
            req.origin_utc_ms,
            &CompileOptions::default(),
        )
        .map_err(|e| format!("{}: {e}", req.key))
    }
}

gx_module!(SystemModule);
