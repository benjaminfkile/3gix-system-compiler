//! Deterministic sine and cosine of an angle in degrees.
//!
//! The registry carries orientation quaternions, and registry bytes go to the
//! hub, so they must be identical on every machine (`matter-format.md`
//! section 1, deterministic bytes). The platform `sin` and `cos` come from
//! the system math library, whose last bit may differ between platforms. This
//! module computes them with `%`, `+`, `-`, `*`, and `/` only, every one of
//! which is correctly rounded in IEEE 754, so the result is bitwise
//! identical everywhere. Accuracy is within a few units in the last place.

use core::f64::consts::PI;

/// Radians per degree.
const RAD_PER_DEG: f64 = PI / 180.0;

/// Returns `(sin(x), cos(x))` for `x` in degrees.
///
/// The angle is reduced to `[0, 360)` with the exact `%` remainder, then to
/// an octant of at most 45 degrees, where Taylor polynomials of degree 17
/// (sine) and 18 (cosine) are accurate to better than 1e-17. A non-finite
/// input returns NaN for both.
pub fn sin_cos_deg(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    let mut d = x % 360.0;
    if d < 0.0 {
        d += 360.0;
    }
    // Quadrant q in 0..=4 and the remainder r in [-45, 45] degrees.
    let q = ((d + 45.0) / 90.0) as i64;
    let r = d - 90.0 * q as f64;
    let t = r * RAD_PER_DEG;
    let (s, c) = (sin_poly(t), cos_poly(t));
    match q.rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// Sine on `[-pi/4, pi/4]`: Taylor series to `t^17`, Horner form.
fn sin_poly(t: f64) -> f64 {
    let t2 = t * t;
    let mut acc = 1.0 / 355_687_428_096_000.0; // 1/17!
    acc = 1.0 / 1_307_674_368_000.0 - t2 * acc; // 1/15!
    acc = 1.0 / 6_227_020_800.0 - t2 * acc; // 1/13!
    acc = 1.0 / 39_916_800.0 - t2 * acc; // 1/11!
    acc = 1.0 / 362_880.0 - t2 * acc; // 1/9!
    acc = 1.0 / 5_040.0 - t2 * acc; // 1/7!
    acc = 1.0 / 120.0 - t2 * acc; // 1/5!
    acc = 1.0 / 6.0 - t2 * acc; // 1/3!
    t - t * t2 * acc
}

/// Cosine on `[-pi/4, pi/4]`: Taylor series to `t^18`, Horner form.
fn cos_poly(t: f64) -> f64 {
    let t2 = t * t;
    let mut acc = 1.0 / 6_402_373_705_728_000.0; // 1/18!
    acc = 1.0 / 20_922_789_888_000.0 - t2 * acc; // 1/16!
    acc = 1.0 / 87_178_291_200.0 - t2 * acc; // 1/14!
    acc = 1.0 / 479_001_600.0 - t2 * acc; // 1/12!
    acc = 1.0 / 3_628_800.0 - t2 * acc; // 1/10!
    acc = 1.0 / 40_320.0 - t2 * acc; // 1/8!
    acc = 1.0 / 720.0 - t2 * acc; // 1/6!
    acc = 1.0 / 24.0 - t2 * acc; // 1/4!
    acc = 0.5 - t2 * acc; // 1/2!
    1.0 - t2 * acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_platform_trig() {
        let mut x = -1000.0;
        while x <= 1000.0 {
            let (s, c) = sin_cos_deg(x);
            let rad = (x % 360.0).to_radians();
            assert!(
                (s - rad.sin()).abs() < 1e-15,
                "sin {x}: {s} vs {}",
                rad.sin()
            );
            assert!(
                (c - rad.cos()).abs() < 1e-15,
                "cos {x}: {c} vs {}",
                rad.cos()
            );
            x += 0.37;
        }
    }

    #[test]
    fn exact_at_quadrant_boundaries() {
        assert_eq!(sin_cos_deg(0.0), (0.0, 1.0));
        assert_eq!(sin_cos_deg(90.0), (1.0, -0.0));
        assert_eq!(sin_cos_deg(180.0), (-0.0, -1.0));
        assert_eq!(sin_cos_deg(270.0), (-1.0, 0.0));
        assert_eq!(sin_cos_deg(360.0), (0.0, 1.0));
        assert_eq!(sin_cos_deg(-90.0), (-1.0, 0.0));
    }

    #[test]
    fn identity_holds() {
        for i in 0..3600 {
            let (s, c) = sin_cos_deg(i as f64 * 0.1 + 0.05);
            assert!((s * s + c * c - 1.0).abs() < 1e-15);
        }
    }

    #[test]
    fn non_finite_is_nan() {
        assert!(sin_cos_deg(f64::NAN).0.is_nan());
        assert!(sin_cos_deg(f64::INFINITY).1.is_nan());
    }
}
