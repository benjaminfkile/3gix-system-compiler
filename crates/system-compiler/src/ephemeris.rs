//! The registry at the origin: the baked state vectors integrated from the
//! data epoch with the library integrator.
//!
//! Implements `space-model.md` section 11 item 1 ("it emits the registry at
//! the origin by integrating its baked state vectors from their epoch to the
//! origin with the library integrator, including oblateness and moments of
//! inertia for every body") and section 7 item 6. The integrator is
//! `gx_core::integrate::advance` with the default
//! [`Scheme::Relativistic`]: Newtonian point masses by fourth order Yoshida,
//! with the first post-Newtonian corrections, the oblateness force of every
//! frame with a `j2`, and the torques on every frame with principal moments
//! as a symmetric perturbation kick.
//!
//! The step is fixed at [`STEP_S`]. `advance` splits the interval into
//! `ceil(|interval| / STEP_S)` equal steps, so the result is a function of
//! the interval alone. The brief for this module asks for a step at which
//! halving it changes the result by less than 1 m. That is not reachable
//! (`docs/model.md`, "Integration step", has the measurements):
//!
//! - The step must stay at or above about 1800 s for the registry of a 2026
//!   origin (27 years from the data epoch) to compute inside the worker's
//!   60 s time limit (`client-compute.md` section 4) under the native host,
//!   which runs this module about 8 times slower than native code.
//! - Truncation error at 1800 s is a few hundred meters for the satellite
//!   and the innermost planet (halving changes them by 304 m and 113 m),
//!   but halving again does not shrink the change: at 900 s and below it
//!   stays at tens to hundreds of meters and grows as the step shrinks.
//! - That floor is rounding, not truncation. Changing the step from 1800 s
//!   to 1799.9 s, which moves the truncation error by about 2e-4 of itself,
//!   already moves bodies by up to 10 m: the integrator carries every state
//!   in `f64` barycentric coordinates, and the rounding of about 470 000
//!   steps accumulates into an along-track drift that depends on the exact
//!   step sequence. Only a change to the core integrator could remove it.
//!
//! [`STEP_S`] is therefore the finest step the time limit allows. The
//! registry it gives is within 3.4e-5 of its orbit radius of the published
//! ephemeris for every body at the test origin (`tests/ephemeris.rs`).
//! `tests/ephemeris.rs` also checks the brief's 1 m halving criterion and
//! reports how far the result is from it.
//!
//! Everything the integrator does is bitwise deterministic
//! (core `docs/determinism.md`), so the registry bytes are too.

use gx_core::frames::FrameSystem;
use gx_core::integrate::{advance, Scheme};
use gx_core::registry::{FrameTree, Registry};
use gx_core::units::Seconds;

/// The integration step, seconds: 1/48 of a day.
pub const STEP_S: f64 = 1800.0;

/// The farthest the origin may be from the data epoch, seconds: 50 Julian
/// years. Integrating farther at [`STEP_S`] would exceed the worker's 60 s
/// time limit (`client-compute.md` section 4) under the native host.
pub const MAX_INTERVAL_S: f64 = 50.0 * 365.25 * 86_400.0;

/// The scheme the registry is integrated with.
pub const SCHEME: Scheme = Scheme::Relativistic;

/// Why a registry could not be integrated.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum EphemerisError {
    /// The origin is too far from the data epoch, or not finite.
    #[error(
        "origin {origin_tdb_s} s TDB is more than {MAX_INTERVAL_S} s from the data epoch {epoch_s} s"
    )]
    OutOfRange {
        /// The origin in TDB seconds since J2000.
        origin_tdb_s: f64,
        /// The data epoch in TDB seconds since J2000.
        epoch_s: f64,
    },
}

/// Integrates the frames of `epoch_registry` from its epoch to `to` (TDB
/// seconds since J2000) at [`STEP_S`] with [`SCHEME`].
pub fn integrate(epoch_registry: &Registry, to: Seconds) -> Result<FrameSystem, EphemerisError> {
    integrate_with_step(epoch_registry, to, Seconds::new(STEP_S))
}

/// [`integrate`] with another step, for the step-halving check.
pub fn integrate_with_step(
    epoch_registry: &Registry,
    to: Seconds,
    step: Seconds,
) -> Result<FrameSystem, EphemerisError> {
    let epoch = epoch_registry.epoch();
    let interval = (to - epoch).value();
    if interval.is_nan() || interval.abs() > MAX_INTERVAL_S {
        return Err(EphemerisError::OutOfRange {
            origin_tdb_s: to.value(),
            epoch_s: epoch.value(),
        });
    }
    let tree = FrameTree::from_registries(std::slice::from_ref(epoch_registry))
        .expect("the epoch registry was checked as a tree at load");
    let mut frames = FrameSystem::from_tree(tree);
    advance(&mut frames, to, step, SCHEME);
    Ok(frames)
}
