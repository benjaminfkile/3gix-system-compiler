//! IAU rotational elements converted to registry orientation and spin.
//!
//! The IAU Working Group on Cartographic Coordinates and Rotational Elements
//! gives, for each body, the right ascension `alpha0` and declination
//! `delta0` of its north pole in ICRF axes, the angle `W` of its prime
//! meridian measured along its equator from the ascending node of that
//! equator on the ICRF equator, and the rate `Wdot` (negative for retrograde
//! rotation). The body-fixed axes are then
//!
//! ```text
//! body to ICRF = Rz(alpha0 + 90 deg) * Rx(90 deg - delta0) * Rz(W)
//! ```
//!
//! with each `R` an active right-handed rotation: body `z` is the pole and
//! body `x` points at the prime meridian. The registry (`matter-format.md`
//! section 5.2) stores this as a unit quaternion `(x, y, z, w)` giving the
//! frame axes relative to the parent axes at the epoch, and an angular
//! velocity in radians per second about the frame axes, here `(0, 0,
//! Wdot)`. [`RotationElements::orientation`] is relative to ICRF axes;
//! [`crate::model::System`] composes it with the parent's to get the value
//! relative to the parent. Sine and cosine come from [`crate::detmath`], so
//! the quaternion bits are identical on every machine.

use gx_core::units::{Quat, RadiansPerSecond, Vec3};

use crate::detmath::sin_cos_deg;

/// Seconds in one day of 86400 SI seconds, the IAU day for `Wdot`.
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// IAU rotational elements of one body at the epoch.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RotationElements {
    /// Right ascension of the north pole in ICRF axes, degrees.
    pub pole_ra_deg: f64,
    /// Declination of the north pole in ICRF axes, degrees.
    pub pole_dec_deg: f64,
    /// Prime meridian angle `W` at the epoch, degrees.
    pub prime_meridian_deg: f64,
    /// Rotation rate `Wdot`, degrees per day; negative for retrograde.
    pub rate_deg_per_day: f64,
}

impl RotationElements {
    /// Body axes relative to ICRF axes at the epoch, as a unit quaternion.
    pub fn orientation(&self) -> Quat {
        let node = about_z(self.pole_ra_deg + 90.0);
        let tilt = about_x(90.0 - self.pole_dec_deg);
        let spin = about_z(self.prime_meridian_deg);
        node * tilt * spin
    }

    /// The rotation rate converted to radians per second.
    pub fn rate(&self) -> RadiansPerSecond {
        RadiansPerSecond::new(self.rate_deg_per_day.to_radians() / SECONDS_PER_DAY)
    }

    /// Angular velocity about the body axes: the rate about body `z`, in
    /// radians per second.
    pub fn angular_velocity(&self) -> Vec3 {
        Vec3::new(0.0, 0.0, self.rate().value())
    }

    /// Unit vector of the north pole in ICRF axes:
    /// `(cos d cos a, cos d sin a, sin d)`.
    pub fn pole(&self) -> Vec3 {
        let (sa, ca) = sin_cos_deg(self.pole_ra_deg);
        let (sd, cd) = sin_cos_deg(self.pole_dec_deg);
        Vec3::new(cd * ca, cd * sa, sd)
    }

    /// Sidereal rotation period in seconds: `360 / |Wdot|` days.
    pub fn sidereal_rotation_period_s(&self) -> f64 {
        360.0 / self.rate_deg_per_day.abs() * SECONDS_PER_DAY
    }
}

/// Active rotation by `deg` degrees about `+x`.
fn about_x(deg: f64) -> Quat {
    let (s, c) = sin_cos_deg(0.5 * deg);
    Quat::new(s, 0.0, 0.0, c)
}

/// Active rotation by `deg` degrees about `+z`.
fn about_z(deg: f64) -> Quat {
    let (s, c) = sin_cos_deg(0.5 * deg);
    Quat::new(0.0, 0.0, s, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

    fn elements(ra: f64, dec: f64, w: f64, rate: f64) -> RotationElements {
        RotationElements {
            pole_ra_deg: ra,
            pole_dec_deg: dec,
            prime_meridian_deg: w,
            rate_deg_per_day: rate,
        }
    }

    fn close(a: Vec3, b: Vec3, tol: f64) -> bool {
        (a - b).length() <= tol
    }

    /// The pole computed with the platform trigonometry, independently of
    /// `detmath`.
    fn pole_reference(ra: f64, dec: f64) -> Vec3 {
        let (a, d) = (ra.to_radians(), dec.to_radians());
        Vec3::new(d.cos() * a.cos(), d.cos() * a.sin(), d.sin())
    }

    #[test]
    fn pole_on_icrf_z_with_zero_angles_is_a_turn_about_z() {
        // alpha0 = 0, delta0 = 90: the node is at RA 90 deg, so body x is
        // the ICRF y axis rotated by W.
        let q = elements(0.0, 90.0, 0.0, 0.0).orientation();
        assert!(q.is_unit(1e-15));
        assert!(close(q.rotate(Z), Z, 1e-15));
        assert!(close(q.rotate(X), Vec3::new(0.0, 1.0, 0.0), 1e-15));
        let q = elements(0.0, 90.0, 90.0, 0.0).orientation();
        assert!(close(q.rotate(X), Vec3::new(-1.0, 0.0, 0.0), 1e-15));
    }

    #[test]
    fn body_z_goes_to_pole() {
        for &(ra, dec) in &[
            (0.0, 0.0),
            (286.13, 63.87),
            (257.311, -15.175),
            (40.589, 83.537),
            (123.0, -89.0),
        ] {
            let q = elements(ra, dec, 17.0, 1.0).orientation();
            assert!(q.is_unit(1e-15));
            let pole = pole_reference(ra, dec);
            assert!(close(q.rotate(Z), pole, 1e-15), "ra {ra} dec {dec}");
            assert!(close(elements(ra, dec, 0.0, 0.0).pole(), pole, 1e-15));
        }
    }

    #[test]
    fn body_x_is_at_w_from_the_node() {
        let (ra, dec, w): (f64, f64, f64) = (272.76, 67.16, 160.2);
        let q = elements(ra, dec, w, -1.48).orientation();
        let pole = pole_reference(ra, dec);
        // Ascending node of the body equator on the ICRF equator: RA + 90.
        let node = Vec3::new(
            (ra + 90.0).to_radians().cos(),
            (ra + 90.0).to_radians().sin(),
            0.0,
        );
        let expected =
            node.scale(w.to_radians().cos()) + pole.cross(node).scale(w.to_radians().sin());
        assert!(close(q.rotate(X), expected, 1e-15));
    }

    #[test]
    fn rate_converts_degrees_per_day_to_radians_per_second() {
        let e = elements(0.0, 90.0, 0.0, 360.0);
        let expected = 2.0 * core::f64::consts::PI / 86_400.0;
        assert!((e.rate().value() - expected).abs() < 1e-20);
        assert_eq!(e.angular_velocity(), Vec3::new(0.0, 0.0, e.rate().value()));
        assert!((e.sidereal_rotation_period_s() - 86_400.0).abs() < 1e-9);
        let retro = elements(0.0, 90.0, 0.0, -1.4813688);
        assert!(retro.rate().value() < 0.0);
    }
}
