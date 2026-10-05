//! Two-body osculating orbit elements from a state vector.
//!
//! Given the position and velocity of a body relative to a central mass, the
//! vis-viva equation gives the semi-major axis from the orbital energy,
//!
//! ```text
//! a = 1 / (2 / r - v^2 / mu),   mu = G * M,
//! ```
//!
//! the eccentricity vector gives the shape,
//!
//! ```text
//! e = ((v^2 - mu / r) r - (r . v) v) / mu,
//! ```
//!
//! and Kepler's third law gives the period `T = 2 pi sqrt(a^3 / mu)`. The
//! compiler never emits these; they exist to check that the ephemeris in
//! `data/system.toml` is self-consistent and agrees with the integrator
//! (`space-model.md` section 11, milestone 1). `G` is
//! [`gx_core::gravity::G`], the same constant the integrator uses.

use core::f64::consts::PI;

use gx_core::gravity::G;
use gx_core::units::{Kilograms, Meters, Seconds, Vec3};

/// The osculating ellipse of a bound two-body orbit.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ellipse {
    /// Semi-major axis.
    pub semi_major_axis: Meters,
    /// Eccentricity, 0 for a circle and below 1 for any bound orbit.
    pub eccentricity: f64,
    /// Orbital period from Kepler's third law.
    pub period: Seconds,
    /// Closest distance to the central mass, `a (1 - e)`.
    pub periapsis: Meters,
    /// Farthest distance from the central mass, `a (1 + e)`.
    pub apoapsis: Meters,
}

/// Osculating elements of a body at `position` (meters) moving at `velocity`
/// (meters per second), both relative to a central mass `central`. Returns
/// `None` if the orbit is not bound (energy at or above zero) or the inputs
/// are degenerate.
pub fn osculating(position: Vec3, velocity: Vec3, central: Kilograms) -> Option<Ellipse> {
    let mu = G * central.value();
    let r = position.length();
    let v2 = velocity.length_squared();
    if !(mu > 0.0 && r > 0.0 && r.is_finite() && v2.is_finite()) {
        return None;
    }
    let inverse_a = 2.0 / r - v2 / mu;
    if inverse_a <= 0.0 {
        return None;
    }
    let a = 1.0 / inverse_a;
    let e_vec =
        (position.scale(v2 - mu / r) - velocity.scale(position.dot(velocity))).scale(1.0 / mu);
    let e = e_vec.length();
    let period = 2.0 * PI * (a * a * a / mu).sqrt();
    Some(Ellipse {
        semi_major_axis: Meters::new(a),
        eccentricity: e,
        period: Seconds::new(period),
        periapsis: Meters::new(a * (1.0 - e)),
        apoapsis: Meters::new(a * (1.0 + e)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_orbit() {
        let m = Kilograms::new(1.0e30);
        let r = 1.0e11;
        let v = (G * m.value() / r).sqrt();
        let o = osculating(Vec3::new(r, 0.0, 0.0), Vec3::new(0.0, v, 0.0), m).unwrap();
        assert!((o.semi_major_axis.value() / r - 1.0).abs() < 1e-12);
        assert!(o.eccentricity < 1e-12);
        let expected = 2.0 * PI * (r * r * r / (G * m.value())).sqrt();
        assert!((o.period.value() / expected - 1.0).abs() < 1e-12);
    }

    #[test]
    fn eccentric_orbit_from_periapsis() {
        // At periapsis q with e = 0.5: v^2 = mu (1 + e) / q.
        let m = Kilograms::new(2.0e30);
        let (q, e) = (5.0e10, 0.5);
        let v = (G * m.value() * (1.0 + e) / q).sqrt();
        let o = osculating(Vec3::new(0.0, q, 0.0), Vec3::new(-v, 0.0, 0.0), m).unwrap();
        assert!((o.eccentricity - e).abs() < 1e-12);
        assert!((o.periapsis.value() / q - 1.0).abs() < 1e-12);
        assert!((o.apoapsis.value() / (q * 3.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn unbound_is_none() {
        let m = Kilograms::new(1.0e30);
        let r = 1.0e11;
        let escape = (2.0 * G * m.value() / r).sqrt();
        let p = Vec3::new(r, 0.0, 0.0);
        assert!(osculating(p, Vec3::new(0.0, escape * 1.01, 0.0), m).is_none());
        assert!(osculating(Vec3::zero(), Vec3::zero(), m).is_none());
    }
}
