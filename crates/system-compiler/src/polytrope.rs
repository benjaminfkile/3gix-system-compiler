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
//! # The atmosphere of a plasma body
//!
//! A body whose temperature is set from its center ([`Thermal::Central`])
//! radiates through an atmosphere. Its temperature is the larger of the
//! polytropic `T_c theta` and the gray Eddington atmosphere
//!
//! ```text
//! T^4 = (3/4) T_eff^4 (tau + 2/3),
//! ```
//!
//! where `tau = kappa * column` is the optical depth from the surface and
//! `column(r) = integral from r to R of rho dr'`. The mass attenuation
//! coefficient `kappa` is set by the photosphere condition: the layer where
//! the polytropic temperature equals the effective temperature sits at
//! `tau = 2/3`, where the Eddington atmosphere has `T = T_eff`. So the
//! interior and the atmosphere meet at the photosphere, and the radiation
//! leaving the surface carries the flux `sigma T_eff^4`.
//!
//! A body whose temperature is set at a reference level
//! ([`Thermal::Reference`]) has its photosphere at that level: `kappa` puts
//! optical depth 2/3 there, so it radiates at the reference temperature. The column is
//! integrated by Simpson's rule, [`COLUMN_STEPS`] sub-steps per table
//! interval and [`PHOTOSPHERE_STEPS`] from the photosphere out.
//!
//! # Radiating temperature
//!
//! A sample's radiating temperature (`matter-format.md` section 3.3) is
//! what its matter sends out of its outer side. For a spherical body the
//! radiance leaving radius `r` outward, from everything below it, is
//!
//! ```text
//! G(r) = integral from tau(r) to infinity of B(T(tau')) e^-(tau' - tau(r)) dtau',
//! ```
//!
//! the formal solution of the transfer equation along the radius, `B` the
//! blackbody radiance summed over the three bands. [`Profile::set_radiation`]
//! tabulates `G` on [`ATMOSPHERE_STEPS`] intervals uniform in `r` from the
//! depth where the optical depth from the surface reaches [`DEEP_TAU`] up to
//! the surface, stepping the recurrence
//! `G(r + dr) = G(r) e^-dtau + B(T_mid) (1 - e^-dtau)` outward; below that
//! depth `G(r) = B(T(r))`. A sample whose matter spans radii `r_in` to
//! `r_out` with optical thickness `dtau` between them sends out
//! `(G(r_out) - e^-dtau G(r_in)) / (1 - e^-dtau)`, and its radiating
//! temperature is the temperature whose summed band radiance equals that
//! ([`Profile::radiating_temperature`]). A uniform sample (`dtau` near 0)
//! gets its own temperature.
//!
//! Only `+`, `-`, `*`, `/`, `sqrt`, and the deterministic
//! `gx_core::detmath::exp` (in `G` and in the band radiance) are used, in a
//! fixed order, so every value is bitwise identical on every machine.

use core::f64::consts::PI;

use std::sync::OnceLock;

use gx_core::detmath::exp;
use gx_core::gravity::G;
use gx_core::radiance::band_radiance;
use gx_core::units::Kelvin;

/// The Runge-Kutta step in `xi`.
pub const XI_STEP: f64 = 1.0 / 2048.0;

/// Intervals of the profile table in `s = (r / R)^2`.
pub const TABLE_LEN: usize = 4096;

/// Simpson sub-steps of the column integral per table interval (even).
pub const COLUMN_STEPS: usize = 8;

/// Simpson sub-steps of the column integral from the photosphere to the
/// surface (even).
pub const PHOTOSPHERE_STEPS: usize = 4096;

/// Optical depth of the photosphere in the Eddington atmosphere.
pub const PHOTOSPHERE_TAU: f64 = 2.0 / 3.0;

/// Intervals of the radiation table from [`DEEP_TAU`] to the surface.
pub const ATMOSPHERE_STEPS: usize = 4096;

/// Optical depth below which the radiation table starts: deeper matter
/// sends out its own blackbody radiance, `e^-DEEP_TAU` of what lies below
/// it reaching the top.
pub const DEEP_TAU: f64 = 60.0;

/// Points of the blackbody inversion table, uniform in `T^(1/4)`.
const INVERSE_POINTS: usize = 16384;

/// Largest `T^(1/4)` of the inversion table: 128, so `T` up to 2.7e8 K.
const INVERSE_TOP: f64 = 128.0;

/// Summed band radiance at `u^4` for `u = INVERSE_TOP * k / (INVERSE_POINTS
/// - 1)`, increasing in `k`.
fn inverse_table() -> &'static [f64] {
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..INVERSE_POINTS)
            .map(|k| {
                let u = INVERSE_TOP * k as f64 / (INVERSE_POINTS - 1) as f64;
                radiance_sum(u * u * u * u)
            })
            .collect()
    })
}

/// Blackbody radiance summed over the three bands, W m^-2 sr^-1.
fn radiance_sum(t: f64) -> f64 {
    let b = band_radiance(Kelvin::new(t));
    b[0] + b[1] + b[2]
}

/// The temperature whose summed band radiance is `b`: binary search in the
/// inversion table and linear interpolation in `T^(1/4)`.
fn temperature_of_radiance(b: f64) -> f64 {
    let t = inverse_table();
    if b.is_nan() || b <= 0.0 {
        return 0.0;
    }
    let (mut lo, mut hi) = (0usize, INVERSE_POINTS - 1);
    if b >= t[hi] {
        return INVERSE_TOP * INVERSE_TOP * INVERSE_TOP * INVERSE_TOP;
    }
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if t[mid] <= b {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let f = (b - t[lo]) / (t[hi] - t[lo]);
    let du = INVERSE_TOP / (INVERSE_POINTS - 1) as f64;
    let u = (lo as f64 + f) * du;
    u * u * u * u
}

/// The radiation table of one profile (module docs).
#[derive(Clone, Debug, PartialEq)]
struct Radiation {
    kappa: f64,
    /// Inner radius of the table, meters.
    r0: f64,
    /// Interval, meters.
    dr: f64,
    /// Column density to the surface at each table radius, kg/m^2.
    column: Vec<f64>,
    /// `G` at each table radius.
    outgoing: Vec<f64>,
}

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
    /// `T = max(T_c theta, T_eddington(tau))`: the ideal-gas polytropic
    /// relation `T ~ P / rho ~ theta` scaled to a central temperature, under
    /// a gray Eddington atmosphere of the effective temperature (module
    /// docs). The mass attenuation coefficient follows from the
    /// photosphere condition.
    Central {
        /// Temperature at the center, kelvin.
        central: f64,
        /// Effective temperature, kelvin: the temperature at the
        /// photosphere, optical depth 2/3.
        effective: f64,
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
    /// The mass attenuation coefficient set by the photosphere condition,
    /// m^2/kg.
    attenuation: Option<f64>,
    /// Temperature at the surface, kelvin.
    surface_temperature: f64,
    /// Column density from each table point to the surface, kg/m^2.
    column: Vec<f64>,
    radiation: Option<Radiation>,
    xi1: f64,
}

/// Why a profile could not be built.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ProfileError {
    /// The reference pressure is not reached inside the body, or the index
    /// has no closed form for it (only index 1 does).
    #[error("the reference level is not inside the body: {0}")]
    Reference(&'static str),
    /// The effective temperature is not between 0 and the central one.
    #[error("the effective temperature must be above 0 and below the central temperature")]
    Effective,
}

/// Simpson's rule for `f` over `[a, b]` in `steps` (even) sub-steps.
fn simpson(f: &impl Fn(f64) -> f64, a: f64, b: f64, steps: usize) -> f64 {
    let h = (b - a) / steps as f64;
    let mut sum = f(a) + f(b);
    for k in 1..steps {
        let w = if k % 2 == 1 { 4.0 } else { 2.0 };
        sum += w * f(a + k as f64 * h);
    }
    sum * h / 3.0
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
        let theta_at = |j: usize| -> f64 {
            if j == TABLE_LEN {
                0.0
            } else {
                le.theta(le.xi1() * (j as f64 / TABLE_LEN as f64).sqrt())
            }
        };
        // Column density from each table point to the surface, kg/m^2, and
        // the attenuation from the photosphere condition (module docs).
        let rho = |r: f64| rho_c * power(le.theta(le.xi1() * r / radius), n);
        let r_at = |j: usize| radius * (j as f64 / TABLE_LEN as f64).sqrt();
        let mut column = vec![0.0f64; TABLE_LEN + 1];
        for j in (0..TABLE_LEN).rev() {
            column[j] = column[j + 1] + simpson(&rho, r_at(j), r_at(j + 1), COLUMN_STEPS);
        }
        let attenuation = match thermal {
            Thermal::Central { central, effective } => {
                let theta_eff = effective / central;
                if !(theta_eff > 0.0 && theta_eff < 1.0) {
                    return Err(ProfileError::Effective);
                }
                // theta falls monotonically outward: bisect for the
                // photosphere's radius.
                let (mut lo, mut hi) = (0.0f64, radius);
                for _ in 0..64 {
                    let mid = 0.5 * (lo + hi);
                    if le.theta(le.xi1() * mid / radius) > theta_eff {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let r_eff = 0.5 * (lo + hi);
                let col = simpson(&rho, r_eff, radius, PHOTOSPHERE_STEPS);
                Some(PHOTOSPHERE_TAU / col)
            }
            Thermal::Reference { .. } => {
                // The photosphere is the reference level: theta =
                // reference_theta, found by bisection like the effective
                // level above.
                let (mut lo, mut hi) = (0.0f64, radius);
                for _ in 0..64 {
                    let mid = 0.5 * (lo + hi);
                    if le.theta(le.xi1() * mid / radius) > reference_theta {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let r_ref = 0.5 * (lo + hi);
                let col = simpson(&rho, r_ref, radius, PHOTOSPHERE_STEPS);
                Some(PHOTOSPHERE_TAU / col)
            }
        };
        let temperature = |theta: f64, column: f64| -> f64 {
            match thermal {
                Thermal::Central { central, effective } => {
                    let tau = attenuation.unwrap_or(0.0) * column;
                    let t4 = 0.75 * (tau + PHOTOSPHERE_TAU);
                    (central * theta).max(effective * t4.sqrt().sqrt())
                }
                Thermal::Reference { temperature, .. } => {
                    temperature * (theta / reference_theta).max(1.0)
                }
            }
        };
        let mut density_factor = Vec::with_capacity(TABLE_LEN + 1);
        let mut weighted_temperature = Vec::with_capacity(TABLE_LEN + 1);
        let mut surface_temperature = 0.0;
        let column_table = column.clone();
        for (j, &col) in column.iter().enumerate() {
            let theta = theta_at(j);
            let phi = power(theta, n);
            density_factor.push(phi);
            weighted_temperature.push(phi * temperature(theta, col));
            if j == TABLE_LEN {
                surface_temperature = temperature(theta, col);
            }
        }
        let mut profile = Profile {
            radius,
            central_density: rho_c,
            density_factor,
            weighted_temperature,
            reference_theta,
            attenuation,
            surface_temperature,
            column: column_table,
            radiation: None,
            xi1: le.xi1(),
        };
        if let Some(kappa) = attenuation {
            profile.set_radiation(le, kappa);
        }
        Ok(profile)
    }

    /// Builds the radiation table for the mass attenuation coefficient
    /// `kappa` (module docs); [`Profile::new`] calls it with the derived
    /// coefficient. With `kappa` 0 there is no table and every radiating
    /// temperature is the temperature at the middle of the span.
    fn set_radiation(&mut self, le: &LaneEmden, kappa: f64) {
        if kappa.is_nan() || kappa <= 0.0 {
            self.radiation = None;
            return;
        }
        let radius = self.radius;
        let n = le.index();
        let rho_c = self.central_density;
        let rho = |r: f64| rho_c * power(le.theta(le.xi1() * r / radius), n);
        // The deepest table radius: where kappa * column reaches DEEP_TAU,
        // bisected on the column table (column falls outward).
        let deep = DEEP_TAU / kappa;
        let r0 = if self.column[0] <= deep {
            0.0
        } else {
            let (mut lo, mut hi) = (0usize, TABLE_LEN);
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if self.column[mid] > deep {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            radius * (lo as f64 / TABLE_LEN as f64).sqrt()
        };
        let dr = (radius - r0) / ATMOSPHERE_STEPS as f64;
        let r_at = |k: usize| {
            if k == ATMOSPHERE_STEPS {
                radius
            } else {
                r0 + k as f64 * dr
            }
        };
        let mut column = vec![0.0f64; ATMOSPHERE_STEPS + 1];
        for k in (0..ATMOSPHERE_STEPS).rev() {
            column[k] = column[k + 1] + simpson(&rho, r_at(k), r_at(k + 1), COLUMN_STEPS);
        }
        let mut outgoing = Vec::with_capacity(ATMOSPHERE_STEPS + 1);
        let mut g = radiance_sum(self.temperature_at(r0));
        outgoing.push(g);
        for k in 0..ATMOSPHERE_STEPS {
            let dtau = kappa * (column[k] - column[k + 1]);
            let through = exp(-dtau);
            let mid = 0.5 * (r_at(k) + r_at(k + 1));
            g = g * through + radiance_sum(self.temperature_at(mid)) * (1.0 - through);
            outgoing.push(g);
        }
        self.radiation = Some(Radiation {
            kappa,
            r0,
            dr,
            column,
            outgoing,
        });
    }

    /// The radiating temperature of matter spanning radii `r_in` to
    /// `r_out` (meters from the center, `r_out` clamped to the surface),
    /// kelvin (module docs). Without a radiation table it is the
    /// temperature at the middle of the span.
    pub fn radiating_temperature(&self, r_in: f64, r_out: f64) -> f64 {
        let r_out = r_out.min(self.radius);
        let r_in = r_in.min(r_out);
        let Some(rad) = &self.radiation else {
            return self.temperature_at(0.5 * (r_in + r_out));
        };
        if r_out <= rad.r0 {
            return self.temperature_at(r_out);
        }
        // Linear interpolation in the table.
        let at = |table: &[f64], r: f64| -> f64 {
            let u = (r - rad.r0) / rad.dr;
            let k = (u as usize).min(ATMOSPHERE_STEPS - 1);
            let f = u - k as f64;
            table[k] + f * (table[k + 1] - table[k])
        };
        let g_out = at(&rad.outgoing, r_out);
        let tau_out = rad.kappa * at(&rad.column, r_out);
        let b = if r_in <= rad.r0 {
            g_out
        } else {
            let dtau = rad.kappa * at(&rad.column, r_in) - tau_out;
            if dtau < 1e-9 {
                return self.temperature_at(0.5 * (r_in + r_out));
            }
            let through = exp(-dtau);
            (g_out - through * at(&rad.outgoing, r_in)) / (1.0 - through)
        };
        temperature_of_radiance(b)
    }

    /// The body's mean radius, meters: the surface of the profile.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Central density, kilograms per cubic meter.
    pub fn central_density(&self) -> f64 {
        self.central_density
    }

    /// The mass attenuation coefficient set by the photosphere condition,
    /// m^2/kg: optical depth 2/3 at the effective temperature level
    /// ([`Thermal::Central`]) or at the reference level
    /// ([`Thermal::Reference`]).
    pub fn attenuation(&self) -> f64 {
        self.attenuation.unwrap_or(0.0)
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
    /// Inside the last table interval, where the density falls to 0 at the
    /// surface, it is interpolated linearly in `s` between the temperature
    /// at the interval's inner end and the surface temperature.
    pub fn temperature_at(&self, r: f64) -> f64 {
        let x = r / self.radius;
        let s = x * x;
        let last = 1.0 - 1.0 / TABLE_LEN as f64;
        if s < last {
            return self.weighted_temperature(s) / self.density_factor(s);
        }
        let inner = self.weighted_temperature(last) / self.density_factor(last);
        let f = ((s - last) * TABLE_LEN as f64).min(1.0);
        inner + f * (self.surface_temperature - inner)
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
                effective: 5e3,
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
            // Eddington at tau = 0: T_eff (1/2)^(1/4).
            let top = 5e3 * 0.5f64.sqrt().sqrt();
            assert!((p.temperature_at(p.radius()) / top - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn photosphere_at_two_thirds() {
        // The level where T_c theta = T_eff sits at tau = 2/3, so the
        // Eddington atmosphere and the interior meet there at T_eff, and
        // the temperature rises monotonically inward from the surface.
        let le = LaneEmden::solve(3);
        let (central, effective) = (1.571e7, 5772.0);
        let thermal = Thermal::Central { central, effective };
        let p = Profile::new(&le, 1.988e30, 6.957e8, thermal).unwrap();
        let kappa = p.attenuation();
        assert!(kappa > 0.0);
        let r = p.radius();
        let rho = |x: f64| p.density_at(x);
        // Find the photosphere radius from the table's own temperature.
        let (mut lo, mut hi) = (0.9 * r, r);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if p.temperature_at(mid) > effective {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let r_eff = 0.5 * (lo + hi);
        let tau = kappa * simpson(&rho, r_eff, r, 4096);
        assert!((tau / PHOTOSPHERE_TAU - 1.0).abs() < 0.05, "tau {tau}");
        let mut last = 0.0;
        for k in 0..=2000 {
            let t = p.temperature_at(r * (1.0 - k as f64 * 1e-5));
            assert!(t >= last, "temperature falls inward at step {k}");
            last = t;
        }
        assert!(Profile::new(
            &le,
            1.988e30,
            6.957e8,
            Thermal::Central {
                central,
                effective: 2e7
            }
        )
        .is_err());
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
