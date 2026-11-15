//! IAU rotational elements evaluated at a time and converted to registry
//! orientation and spin.
//!
//! The IAU Working Group on Cartographic Coordinates and Rotational Elements
//! gives, for each body, the right ascension `alpha0` and declination
//! `delta0` of its north pole in ICRF axes and the angle `W` of its prime
//! meridian measured along its equator from the ascending node of that
//! equator on the ICRF equator, each as a polynomial in time plus periodic
//! (nutation precession) terms:
//!
//! ```text
//! alpha0 = a0 + a1 T + sum ra_i  sin(theta_i)
//! delta0 = d0 + d1 T + sum dec_i cos(theta_i)
//! W      = W0 + W1 d + W2 d^2 + sum pm_i sin(theta_i)
//! theta_i = t0 + t1 T + t2 T^2
//! ```
//!
//! with `T` Julian centuries and `d` days of TDB since J2000. The body-fixed
//! axes at that moment are
//!
//! ```text
//! body to ICRF = Rz(alpha0 + 90 deg) * Rx(90 deg - delta0) * Rz(W)
//! ```
//!
//! with each `R` an active right-handed rotation: body `z` is the pole and
//! body `x` points at the prime meridian. The registry (`matter-format.md`
//! section 5.2) stores this as a unit quaternion giving the frame axes
//! relative to the parent axes, and an angular velocity in radians per
//! second about the frame axes, here `(0, 0, dW/dt)`: the time derivative of
//! the whole `W` expression, periodic terms included. The slow motion of the
//! pole itself (below 1e-11 rad/s for every body here) is not added to the
//! spin. Sine and cosine come from [`crate::detmath`], so the quaternion bits
//! are identical on every machine.
//!
//! This implements the "orientation and spin at the origin" half of the
//! registry of `space-model.md` section 11 item 1: the module evaluates these
//! elements at the origin (the positions come from the integrator,
//! [`crate::ephemeris`]).

use gx_core::units::{Quat, RadiansPerSecond, Seconds, Vec3};

use crate::detmath::sin_cos_deg;

/// Seconds in one day of 86400 SI seconds, the IAU day for `d` and `W1`.
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// Days in one Julian century, the IAU century for `T`.
pub const DAYS_PER_CENTURY: f64 = 36_525.0;

/// One nutation precession term of a body's rotational elements.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PeriodicTerm {
    /// The term's angle `[t0, t1, t2]`: degrees, degrees per Julian
    /// century, degrees per Julian century squared.
    pub angle_deg: [f64; 3],
    /// Coefficient of `sin(theta)` in the pole right ascension, degrees.
    pub ra_deg: f64,
    /// Coefficient of `cos(theta)` in the pole declination, degrees.
    pub dec_deg: f64,
    /// Coefficient of `sin(theta)` in the prime meridian angle, degrees.
    pub pm_deg: f64,
}

/// The IAU rotational elements of one body, valid at any time.
#[derive(Clone, Debug, PartialEq)]
pub struct RotationElements {
    /// Pole right ascension `[a0, a1]`: degrees, degrees per Julian century.
    pub pole_ra_deg: [f64; 2],
    /// Pole declination `[d0, d1]`: degrees, degrees per Julian century.
    pub pole_dec_deg: [f64; 2],
    /// Prime meridian `[W0, W1, W2]`: degrees, degrees per day, degrees per
    /// day squared. `W1` is negative for retrograde rotation.
    pub prime_meridian_deg: [f64; 3],
    /// Periodic terms, in data file order.
    pub terms: Vec<PeriodicTerm>,
}

/// Rotational elements evaluated at one moment.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Attitude {
    /// Right ascension of the north pole in ICRF axes, degrees.
    pub pole_ra_deg: f64,
    /// Declination of the north pole in ICRF axes, degrees.
    pub pole_dec_deg: f64,
    /// Prime meridian angle `W`, degrees.
    pub prime_meridian_deg: f64,
    /// `dW/dt`, degrees per day; negative for retrograde rotation.
    pub rate_deg_per_day: f64,
}

impl RotationElements {
    /// The elements evaluated at `tdb`, seconds of TDB since J2000.
    ///
    /// The sums run in data file order with plain operators and
    /// [`sin_cos_deg`], so the result is bitwise identical everywhere.
    pub fn at(&self, tdb: Seconds) -> Attitude {
        let d = tdb.value() / SECONDS_PER_DAY;
        let t = d / DAYS_PER_CENTURY;
        let [a0, a1] = self.pole_ra_deg;
        let [d0, d1] = self.pole_dec_deg;
        let [w0, w1, w2] = self.prime_meridian_deg;
        let mut ra = a0 + a1 * t;
        let mut dec = d0 + d1 * t;
        let mut w = w0 + w1 * d + w2 * d * d;
        let mut rate = w1 + 2.0 * w2 * d;
        for term in &self.terms {
            let [t0, t1, t2] = term.angle_deg;
            let theta = t0 + t1 * t + t2 * t * t;
            // d theta / d day, degrees per day.
            let theta_rate = (t1 + 2.0 * t2 * t) / DAYS_PER_CENTURY;
            let (s, c) = sin_cos_deg(theta);
            ra += term.ra_deg * s;
            dec += term.dec_deg * c;
            w += term.pm_deg * s;
            rate += term.pm_deg * c * theta_rate.to_radians();
        }
        Attitude {
            pole_ra_deg: ra,
            pole_dec_deg: dec,
            prime_meridian_deg: w,
            rate_deg_per_day: rate,
        }
    }
}

impl Attitude {
    /// Body axes relative to ICRF axes, as a unit quaternion.
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

    /// Sidereal rotation period in seconds: `360 / |dW/dt|` days.
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

    fn attitude(ra: f64, dec: f64, w: f64, rate: f64) -> Attitude {
        Attitude {
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
        let q = attitude(0.0, 90.0, 0.0, 0.0).orientation();
        assert!(q.is_unit(1e-15));
        assert!(close(q.rotate(Z), Z, 1e-15));
        assert!(close(q.rotate(X), Vec3::new(0.0, 1.0, 0.0), 1e-15));
        let q = attitude(0.0, 90.0, 90.0, 0.0).orientation();
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
            let q = attitude(ra, dec, 17.0, 1.0).orientation();
            assert!(q.is_unit(1e-15));
            let pole = pole_reference(ra, dec);
            assert!(close(q.rotate(Z), pole, 1e-15), "ra {ra} dec {dec}");
            assert!(close(attitude(ra, dec, 0.0, 0.0).pole(), pole, 1e-15));
        }
    }

    #[test]
    fn body_x_is_at_w_from_the_node() {
        let (ra, dec, w): (f64, f64, f64) = (272.76, 67.16, 160.2);
        let q = attitude(ra, dec, w, -1.48).orientation();
        let pole = pole_reference(ra, dec);
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
        let e = attitude(0.0, 90.0, 0.0, 360.0);
        let expected = 2.0 * core::f64::consts::PI / 86_400.0;
        assert!((e.rate().value() - expected).abs() < 1e-20);
        assert_eq!(e.angular_velocity(), Vec3::new(0.0, 0.0, e.rate().value()));
        assert!((e.sidereal_rotation_period_s() - 86_400.0).abs() < 1e-9);
    }

    #[test]
    fn polynomials_and_terms_evaluate() {
        let el = RotationElements {
            pole_ra_deg: [10.0, 2.0],
            pole_dec_deg: [20.0, -1.0],
            prime_meridian_deg: [30.0, 100.0, 1e-6],
            terms: vec![PeriodicTerm {
                angle_deg: [90.0, 36_525.0, 0.0],
                ra_deg: 0.5,
                dec_deg: 0.25,
                pm_deg: 2.0,
            }],
        };
        // At J2000: theta = 90 deg, sin 1, cos 0.
        let a = el.at(Seconds::new(0.0));
        assert_eq!(a.pole_ra_deg, 10.5);
        assert!((a.pole_dec_deg - 20.0).abs() < 1e-15);
        assert_eq!(a.prime_meridian_deg, 32.0);
        assert!((a.rate_deg_per_day - 100.0).abs() < 1e-12);
        // One century later: theta = 90 + 36525 deg = 90 + 165 (mod 360).
        let t = Seconds::new(DAYS_PER_CENTURY * SECONDS_PER_DAY);
        let a = el.at(t);
        let theta = (90.0f64 + 36_525.0).to_radians();
        assert!((a.pole_ra_deg - (12.0 + 0.5 * theta.sin())).abs() < 1e-12);
        assert!((a.pole_dec_deg - (19.0 + 0.25 * theta.cos())).abs() < 1e-12);
        let d = DAYS_PER_CENTURY;
        let w = 30.0 + 100.0 * d + 1e-6 * d * d + 2.0 * theta.sin();
        assert!((a.prime_meridian_deg - w).abs() < 1e-8);
        // dW/dd = W1 + 2 W2 d + pm cos(theta) dtheta/dd (radians).
        let rate = 100.0 + 2e-6 * d + 2.0 * theta.cos() * 1.0f64.to_radians();
        assert!((a.rate_deg_per_day - rate).abs() < 1e-12);
    }
}
