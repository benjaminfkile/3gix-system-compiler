//! Hydrostatic polytropes: the density and temperature profile of a gas or
//! plasma body.
//!
//! Implements the "hydrostatic density profiles (polytropes matching mass
//! and radius)" of `space-model.md` section 11 item 1. A polytrope of index
//! `n` has pressure `P = K rho^(1 + 1/n)` and, in hydrostatic equilibrium,
//! density `rho = rho_c theta^n` where `theta(xi)` solves the Lane-Emden
//! equation
//!
//! ```text
//! theta'' + (2 / xi) theta' + theta^n = 0,   theta(0) = 1, theta'(0) = 0,
//! ```
//!
//! with `r = alpha xi`. The surface is the first zero `xi_1` of `theta`, so
//! `alpha = R / xi_1` for a body of mean radius `R`, and the mass is
//! `M = 4 pi alpha^3 rho_c xi_1^2 |theta'(xi_1)|`, which fixes the central
//! density at `rho_c = rho_mean xi_1 / (3 |theta'(xi_1)|)`.
//!
//! [`LaneEmden::solve`] integrates the equation numerically with a fixed
//! deterministic scheme: the series `theta = 1 - xi^2/6 + n xi^4/120` for the
//! first step off the singular center, then classical fourth order
//! Runge-Kutta at the fixed step [`XI_STEP`] until `theta` changes sign, and
//! 64 bisections of the cubic Hermite interpolant of the last step for the
//! surface. [`Profile`] tabulates `theta` at [`TABLE_LEN`] + 1 points
//! uniform in `s = (r / R)^2` (so a sample point needs no square root) and
//! reads it back by linear interpolation.
//!
//! Only `+`, `-`, `*`, `/`, and `sqrt` are used, in a fixed order, so every
//! value is bitwise identical on every machine.

use core::f64::consts::PI;

use gx_core::gravity::G;

/// The Runge-Kutta step in `xi`.
pub const XI_STEP: f64 = 1.0 / 2048.0;

/// Intervals of the profile table in `s = (r / R)^2`.
pub const TABLE_LEN: usize = 4096;

/// The supported polytropic indices: 1 for the gas bodies, 3 for the plasma
/// body (`space-model.md` section 11 item 1).
pub const INDICES: [u8; 2] = [1, 3];

/// A numerical solution of the Lane-Emden equation of one index.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneEmden {
    n: u8,
    /// `theta` and `theta'` at `xi = k * XI_STEP`, `k = 0, 1, ...`, up to
    /// and including the first node past the surface.
    nodes: Vec<(f64, f64)>,
    xi1: f64,
    slope1: f64,
}

/// `theta^n` for the supported indices, with `theta` clamped at 0.
fn power(theta: f64, n: u8) -> f64 {
    let t = theta.max(0.0);
    match n {
        1 => t,
        _ => t * t * t,
    }
}

/// The cubic Hermite interpolant on one step: value and derivative at
/// `t` in `[0, 1]` of the step from `(y0, m0)` to `(y1, m1)` of length `h`.
fn hermite(y0: f64, m0: f64, y1: f64, m1: f64, h: f64, t: f64) -> (f64, f64) {
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    let y = h00 * y0 + h10 * h * m0 + h01 * y1 + h11 * h * m1;
    let d00 = 6.0 * t2 - 6.0 * t;
    let d10 = 3.0 * t2 - 4.0 * t + 1.0;
    let d01 = -6.0 * t2 + 6.0 * t;
    let d11 = 3.0 * t2 - 2.0 * t;
    let dy = (d00 * y0 + d01 * y1) / h + d10 * m0 + d11 * m1;
    (y, dy)
}

impl LaneEmden {
    /// Integrates the Lane-Emden equation of index `n` (1 or 3) from the
    /// center to the surface.
    ///
    /// # Panics
    ///
    /// Panics if `n` is not in [`INDICES`].
    pub fn solve(n: u8) -> LaneEmden {
        assert!(INDICES.contains(&n), "unsupported polytropic index {n}");
        let nf = f64::from(n);
        let h = XI_STEP;
        let rhs = |xi: f64, y: f64, z: f64| -> (f64, f64) { (z, -power(y, n) - 2.0 * z / xi) };
        let mut nodes = vec![(1.0, 0.0)];
        // First node from the series about the center.
        let (x2, x3) = (h * h, h * h * h);
        let mut y = 1.0 - x2 / 6.0 + nf * x2 * x2 / 120.0;
        let mut z = -h / 3.0 + nf * x3 / 30.0;
        nodes.push((y, z));
        let mut k = 1u32;
        while y > 0.0 {
            let xi = f64::from(k) * h;
            let (a1, b1) = rhs(xi, y, z);
            let (a2, b2) = rhs(xi + 0.5 * h, y + 0.5 * h * a1, z + 0.5 * h * b1);
            let (a3, b3) = rhs(xi + 0.5 * h, y + 0.5 * h * a2, z + 0.5 * h * b2);
            let (a4, b4) = rhs(xi + h, y + h * a3, z + h * b3);
            y += h / 6.0 * (a1 + 2.0 * a2 + 2.0 * a3 + a4);
            z += h / 6.0 * (b1 + 2.0 * b2 + 2.0 * b3 + b4);
            nodes.push((y, z));
            k += 1;
        }
        // The zero lies in the last step: bisect its Hermite interpolant.
        let last = nodes.len() - 1;
        let (y0, m0) = nodes[last - 1];
        let (y1, m1) = nodes[last];
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..64 {
            let mid = 0.5 * (lo + hi);
            if hermite(y0, m0, y1, m1, h, mid).0 > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let t = 0.5 * (lo + hi);
        let xi1 = (f64::from(k - 1) + t) * h;
        let slope1 = hermite(y0, m0, y1, m1, h, t).1;
        LaneEmden {
            n,
            nodes,
            xi1,
            slope1,
        }
    }

    /// The polytropic index.
    pub fn index(&self) -> u8 {
        self.n
    }

    /// The first zero `xi_1` of `theta`: the surface.
    pub fn xi1(&self) -> f64 {
        self.xi1
    }

    /// `theta'(xi_1)`, negative.
    pub fn surface_slope(&self) -> f64 {
        self.slope1
    }

    /// `theta(xi)` for `xi` in `[0, xi_1]` by the cubic Hermite interpolant
    /// of the step holding `xi`; 0 at and beyond the surface.
    pub fn theta(&self, xi: f64) -> f64 {
        if xi >= self.xi1 {
            return 0.0;
        }
        let u = xi.max(0.0) / XI_STEP;
        let k = (u as usize).min(self.nodes.len() - 2);
        let (y0, m0) = self.nodes[k];
        let (y1, m1) = self.nodes[k + 1];
        hermite(y0, m0, y1, m1, XI_STEP, u - k as f64).0.max(0.0)
    }

    /// Central over mean density, `xi_1 / (3 |theta'(xi_1)|)`.
    pub fn central_to_mean(&self) -> f64 {
        self.xi1 / (3.0 * -self.slope1)
    }
}

/// How a polytrope's temperature is set (`docs/model.md`, "Temperature").
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Thermal {
    /// `T = max(T_c theta, floor)`: the ideal-gas polytropic relation
    /// `T ~ P / rho ~ theta` scaled to a central temperature, held at a
    /// floor (the photosphere's effective temperature) in the outer layers.
    Central {
        /// Temperature at the center, kelvin.
        central: f64,
        /// Lowest temperature anywhere inside, kelvin.
        floor: f64,
    },
    /// `T = T_ref max(1, theta / theta_ref)`: the same relation scaled so
    /// that the level where the model's own pressure is `pressure` has the
    /// temperature `temperature`; layers above that level keep it.
    Reference {
        /// Pressure of the reference level, pascals.
        pressure: f64,
        /// Temperature at the reference level, kelvin.
        temperature: f64,
    },
}

/// The tabulated profile of one body: a polytrope matching its mass and
/// mean radius.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    radius: f64,
    central_density: f64,
    /// `theta^n` at `s = j / TABLE_LEN`, `j = 0 ..= TABLE_LEN`.
    density_factor: Vec<f64>,
    /// `theta^n * T` at the same points, kelvin.
    weighted_temperature: Vec<f64>,
    /// `theta` at the reference level, for [`Thermal::Reference`]; 1 for
    /// [`Thermal::Central`].
    reference_theta: f64,
    xi1: f64,
}

/// Why a profile could not be built.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ProfileError {
    /// The reference pressure is not reached inside the body, or the index
    /// has no closed form for it (only index 1 does).
    #[error("the reference level is not inside the body: {0}")]
    Reference(&'static str),
}

/// Linear interpolation in a table uniform in `s` over `[0, 1]`.
fn lookup(table: &[f64], s: f64) -> f64 {
    let u = s * TABLE_LEN as f64;
    let i = u as usize;
    if i >= TABLE_LEN {
        return table[TABLE_LEN];
    }
    let f = u - i as f64;
    table[i] + f * (table[i + 1] - table[i])
}

impl Profile {
    /// Builds the profile of a body of `mass` (kilograms) and mean `radius`
    /// (meters) as a polytrope of the solution `le`, with temperatures from
    /// `thermal`.
    pub fn new(
        le: &LaneEmden,
        mass: f64,
        radius: f64,
        thermal: Thermal,
    ) -> Result<Profile, ProfileError> {
        let mean = mass / (4.0 / 3.0 * PI * radius * radius * radius);
        let rho_c = mean * le.central_to_mean();
        let n = le.index();
        let reference_theta = match thermal {
            Thermal::Central { .. } => 1.0,
            Thermal::Reference { pressure, .. } => {
                if n != 1 {
                    return Err(ProfileError::Reference(
                        "a reference pressure needs polytropic index 1",
                    ));
                }
                // Index 1: K = 2 pi G alpha^2 and P = K rho_c^2 theta^2.
                let alpha = radius / le.xi1();
                let k = 2.0 * PI * G * alpha * alpha;
                let theta = (pressure / (k * rho_c * rho_c)).sqrt();
                if !(theta > 0.0 && theta < 1.0) {
                    return Err(ProfileError::Reference(
                        "the model's central pressure is below the reference pressure",
                    ));
                }
                theta
            }
        };
        let temperature = |theta: f64| -> f64 {
            match thermal {
                Thermal::Central { central, floor } => (central * theta).max(floor),
                Thermal::Reference { temperature, .. } => {
                    temperature * (theta / reference_theta).max(1.0)
                }
            }
        };
        let mut density_factor = Vec::with_capacity(TABLE_LEN + 1);
        let mut weighted_temperature = Vec::with_capacity(TABLE_LEN + 1);
        for j in 0..=TABLE_LEN {
            let s = j as f64 / TABLE_LEN as f64;
            let theta = if j == TABLE_LEN {
                0.0
            } else {
                le.theta(le.xi1() * s.sqrt())
            };
            let phi = power(theta, n);
            density_factor.push(phi);
            weighted_temperature.push(phi * temperature(theta));
        }
        Ok(Profile {
            radius,
            central_density: rho_c,
            density_factor,
            weighted_temperature,
            reference_theta,
            xi1: le.xi1(),
        })
    }

    /// The body's mean radius, meters: the surface of the profile.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Central density, kilograms per cubic meter.
    pub fn central_density(&self) -> f64 {
        self.central_density
    }

    /// `theta` at the reference level of [`Thermal::Reference`], 1 for
    /// [`Thermal::Central`].
    pub fn reference_theta(&self) -> f64 {
        self.reference_theta
    }

    /// The radius of the reference level, meters: where `theta` equals
    /// [`Profile::reference_theta`], found by bisection on the table.
    pub fn reference_radius(&self) -> f64 {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..64 {
            let mid = 0.5 * (lo + hi);
            if lookup(&self.density_factor, mid) > self.reference_density_factor() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        self.radius * (0.5 * (lo + hi)).sqrt()
    }

    fn reference_density_factor(&self) -> f64 {
        // Index 1 is the only one with a reference level: theta^1.
        self.reference_theta
    }

    /// `theta^n` at `s = (r / R)^2`, by linear interpolation; 0 for `s` at
    /// or beyond 1.
    pub fn density_factor(&self, s: f64) -> f64 {
        lookup(&self.density_factor, s)
    }

    /// `theta^n T` at `s = (r / R)^2`, kelvin: the density-weighted
    /// temperature, by linear interpolation.
    pub fn weighted_temperature(&self, s: f64) -> f64 {
        lookup(&self.weighted_temperature, s)
    }

    /// Density at distance `r` from the center, kilograms per cubic meter.
    pub fn density_at(&self, r: f64) -> f64 {
        let x = r / self.radius;
        self.central_density * self.density_factor(x * x)
    }

    /// Temperature at distance `r` from the center inside the body, kelvin.
    /// At the surface, where the density is 0, it is the limit from inside
    /// of the last table interval.
    pub fn temperature_at(&self, r: f64) -> f64 {
        let x = r / self.radius;
        let s = (x * x).min(1.0 - 1.0 / TABLE_LEN as f64);
        self.weighted_temperature(s) / self.density_factor(s)
    }

    /// `xi_1` of the underlying solution.
    pub fn xi1(&self) -> f64 {
        self.xi1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_one_matches_the_closed_form() {
        // theta = sin(xi) / xi, xi_1 = pi, theta'(pi) = -1 / pi.
        let le = LaneEmden::solve(1);
        assert!((le.xi1() - PI).abs() < 1e-9, "{}", le.xi1());
        assert!((le.surface_slope() + 1.0 / PI).abs() < 1e-9);
        for &xi in &[0.0, 0.3, 1.0, 2.0, 3.0] {
            let exact = if xi == 0.0 { 1.0 } else { f64::sin(xi) / xi };
            assert!((le.theta(xi) - exact).abs() < 1e-9, "xi {xi}");
        }
        assert!((le.central_to_mean() - PI * PI / 3.0).abs() < 1e-8);
    }

    #[test]
    fn index_three_matches_published_constants() {
        // Chandrasekhar (1939), table 4: xi_1 = 6.89685, -xi_1^2 theta'(xi_1)
        // = 2.01824, rho_c / rho_mean = 54.1825.
        let le = LaneEmden::solve(3);
        assert!((le.xi1() - 6.89685).abs() < 1e-5, "{}", le.xi1());
        let m = -le.xi1() * le.xi1() * le.surface_slope();
        assert!((m - 2.01824).abs() < 1e-5, "{m}");
        assert!((le.central_to_mean() - 54.1825).abs() < 1e-3);
    }

    #[test]
    fn deterministic() {
        assert_eq!(LaneEmden::solve(3), LaneEmden::solve(3));
    }

    #[test]
    fn profile_integrates_to_the_mass() {
        for n in INDICES {
            let le = LaneEmden::solve(n);
            let thermal = Thermal::Central {
                central: 1e7,
                floor: 5e3,
            };
            let p = Profile::new(&le, 2.0e30, 7.0e8, thermal).unwrap();
            // Midpoint rule over shells, finer than the table.
            let shells = 100_000;
            let dr = p.radius() / shells as f64;
            let mut m = 0.0;
            for i in 0..shells {
                let r = (i as f64 + 0.5) * dr;
                m += 4.0 * PI * r * r * dr * p.density_at(r);
            }
            assert!((m / 2.0e30 - 1.0).abs() < 1e-4, "index {n}: {m:e}");
            assert_eq!(p.density_at(p.radius()), 0.0);
            assert!((p.temperature_at(0.0) - 1e7).abs() < 1.0);
            assert_eq!(p.temperature_at(p.radius()), 5e3);
        }
    }

    #[test]
    fn reference_level() {
        let le = LaneEmden::solve(1);
        let thermal = Thermal::Reference {
            pressure: 1e5,
            temperature: 165.0,
        };
        let p = Profile::new(&le, 1.9e27, 7.0e7, thermal).unwrap();
        let theta = p.reference_theta();
        assert!(theta > 0.0 && theta < 1.0);
        // The level is near the surface, inside the last table intervals,
        // where the linear table is coarse.
        let r = p.reference_radius();
        assert!(r > 0.999 * p.radius() && r < p.radius());
        let t = p.temperature_at(r);
        assert!((t / 165.0 - 1.0).abs() < 0.5, "{t} at {r} theta {theta}");
        // Inside, T = T_ref theta / theta_ref: theta(pi / 2) = 2 / pi.
        let t = p.temperature_at(0.5 * p.radius());
        let expected = 165.0 * (2.0 / PI) / theta;
        assert!((t / expected - 1.0).abs() < 1e-6, "{t} vs {expected}");
        assert_eq!(p.temperature_at(p.radius()), 165.0);
        assert!((p.temperature_at(0.0) - 165.0 / theta).abs() < 1e-6 * 165.0 / theta);
        assert!(Profile::new(&LaneEmden::solve(3), 1.9e27, 7.0e7, thermal).is_err());
        // A pressure above the model's central pressure is not inside.
        let deep = Thermal::Reference {
            pressure: 1e30,
            temperature: 165.0,
        };
        assert!(Profile::new(&le, 1.9e27, 7.0e7, deep).is_err());
    }
}
