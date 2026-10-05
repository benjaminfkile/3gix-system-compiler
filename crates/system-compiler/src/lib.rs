//! The system compiler: compiles a planetary system into 3GIX matter.
//!
//! This crate is a compiler in the sense of `space-model.md` section 2. It
//! knows what the bodies of the system are; what it emits does not. This
//! first part holds the physical data and the models built from it:
//!
//! - [`model`]: the parsed and validated contents of `data/system.toml`, the
//!   frame registry built from it (`matter-format.md` section 5), and the
//!   per-body matter model sampled into sections (`matter-format.md`
//!   section 3).
//! - [`rotation`]: IAU rotational elements converted to the registry's
//!   orientation quaternion and angular velocity.
//! - [`orbit`]: two-body osculating elements from a state vector, used to
//!   check the ephemeris against published periods.
//! - [`detmath`]: deterministic sine and cosine, so that registry bytes do
//!   not depend on the platform math library.
//!
//! Milestone 1 of `space-model.md` section 11 is the target: a star, eight
//! orbiting frames, and one frame orbiting the third, with real masses,
//! radii, state vectors, and rotation at the epoch.

#![warn(missing_docs)]

pub mod detmath;
pub mod model;
pub mod orbit;
pub mod rotation;

pub use model::{Body, Matter, ModelError, RootFrame, System};

/// The bundled system data file, `data/system.toml`, baked into the binary
/// so that compiled output never depends on the working directory.
pub const SYSTEM_TOML: &str = include_str!("../../../data/system.toml");
