# How a body becomes matter

This document describes the model the system module uses to turn each body
in `data/system.toml` into a frame in the registry at the universe's origin
and into matter samples. It implements `matter-format.md` sections 3
(matter sections) and 5 (frame registry) and `space-model.md` section 11
item 1. The code is in `crates/system-compiler/src/model.rs`,
`src/ephemeris.rs`, `src/polytrope.rs`, and `src/rotation.rs`.

## Frames

The registry holds eleven frames:

| frame_id | Parent | What |
|---|---|---|
| 0 | none (root) | the solar system barycenter: mass 0, at rest, `root_extent` 2.0e13 m, `max_depth` 0 |
| 1 | 0 | the Sun |
| 2 to 9 | 0 | Mercury, Venus, Earth, Mars, Jupiter, Saturn, Uranus, Neptune |
| 10 | 4 | the Moon |

Each body frame has its origin at the body's center. Its record carries:

- `mass`: GM from DE440 divided by `G`. This is the authoritative
  gravitational mass (`matter-format.md` section 5.2); sampled densities are
  not re-summed to replace it.
- `position`, `velocity`: the state relative to the parent at the origin,
  in ICRF axes, integrated from the baked Horizons states at J2000 (below).
- `orientation`: the body's IAU axes (z on the north pole, x on the prime
  meridian) relative to the parent's axes, from the IAU rotational
  elements evaluated at the origin. For the Moon that is relative to the
  Earth's axes at the origin.
- `angular_velocity`: the time derivative of the IAU elements at the
  origin, in radians per second about the body axes.
- `j2`, `reference_radius`, `principal_moments`: the published zonal
  harmonic, the radius it is quoted against, and the principal moments of
  inertia built from the published `C/(M R^2)` factor as
  `C = factor * M * R^2` and `A = B = C - J2 * M * reference_radius^2`
  (axisymmetric about the pole). Sources are in `data/system.toml`. A body
  without a published J2 gets 0 and constant spin.
- `root_extent`: `8 * mean_radius`, a cube that contains the body with
  room; `max_depth`: 6. The layer's `maxDepth` table is this column
  (`system-compiler max-depth`).

The registry's epoch is the origin in TDB seconds since J2000.

## The registry at the origin

A request carries the space's origin in UTC milliseconds
(`client-compute.md` section 4; the layer runs with `usesOrigin` true). The
module converts it to TDB with `gx_core::timescale::utc_ms_to_tdb_seconds`,
builds the registry at the data epoch (J2000, `[epoch]` in the data file),
and integrates it to the origin with `gx_core::integrate::advance` under
`Scheme::Relativistic`: Newtonian point masses by fourth order Yoshida,
first post-Newtonian corrections, the oblateness force of every frame with
a `j2`, and the torques on every frame with principal moments. Positions and
velocities come from the integration. Orientation and angular velocity are
then replaced by the IAU elements at the origin, so they carry the IAU
precession and nutation rather than the integrated free rotation.

Accuracy (`tests/ephemeris.rs`): at the origin 2026-11-14T00:00:00Z every
body is within 3.4e-5 of its orbit radius of the JPL Horizons position for
that instant, against the brief's 1e-3. The worst in meters is Uranus,
1.2e6 m (4.1e-7 of its orbit radius); the worst relative is the Sun, 2.5e4
m against its 7.7e8 m orbit about the barycenter. The check vectors are in
`crates/system-compiler/tests/data/horizons-2026-11-14.toml`, fetched once
with `scripts/fetch-horizons.py` (`docs/data-sources.md`).

### Integration step

The step is fixed at 1800 s (`ephemeris::STEP_S`). `advance` splits the
interval into `ceil(|interval| / step)` equal steps, so the registry is a
function of the origin alone. Origins more than 50 Julian years from J2000
are refused.

The brief asks for a step at which halving it changes the result by less
than 1 m. No step meets that with the library integrator in `f64` over the
27 years from J2000 to a 2026 origin, and `tests/ephemeris.rs`
(`halving_the_step_changes_the_result_by_less_than_one_meter`) fails for
that reason. Measured at the 2026-11-14 origin, largest position change of
any body when the step is halved:

| Step, s | Relativistic | Newtonian only (`Yoshida4`) | Wall time, native release |
|---|---|---|---|
| 1800 to 900 | 304 m (Moon), 113 m (Mercury) | 294 m (Moon), 124 m (Mercury) | 3.3 s at 1800 |
| 900 to 450 | 152 m (Uranus), 93 m (Moon) | 167 m (Moon), 29 m (Mercury) | 6.8 s at 900 |
| 450 to 225 | 712 m (Neptune), 145 m (Uranus) | | 13.6 s at 450 |

The differences stop shrinking at 900 s and then grow. That is rounding,
not truncation: changing the step from 1800 s to 1799.9 s, which changes
the truncation error by about 2e-4 of itself, moves the Moon by 23 m and
Mercury by 11 m under the Newtonian scheme, and the Moon by 4 m, Mercury
by 7 m, and Venus by 10 m under the relativistic one. The integrator
carries every state in `f64` barycentric coordinates, so each of the
roughly 470 000 steps rounds positions near 1e11 m and velocities near 3e4
m/s, and the rounding accumulates into an along-track drift of meters to
hundreds of meters that depends on the exact step sequence. A smaller step
means more steps and more drift. Getting under 1 m would need a change to
the core integrator (compensated summation or a parent-relative state),
not a different step.

1800 s is kept because the truncation error has fallen below that rounding
floor by then for every body except the Moon and Mercury (their remaining
errors are a few hundred meters, about 1e-6 of their orbit radii), and
because time matters: one registry at a 2026 origin takes about 25 s under
the native module host (wasmtime), whose default time limit is 60 s. At
900 s it would take about 50 s.

## Matter

Every body is a ball of radius `mean_radius` (the radius of the sphere with
the body's volume):

- **State**: `solid` for Mercury, Venus, Earth, Mars, and the Moon; `gas`
  for Jupiter, Saturn, Uranus, and Neptune; `plasma` for the Sun.
- **Density, solid bodies**: uniform, the mean density
  `mass / (4/3 pi mean_radius^3)`, computed in code and never stored.
- **Density, gas and plasma bodies**: a hydrostatic polytrope matching the
  body's mass and mean radius, index 1 for the gas bodies and index 3 for
  the plasma body (below).
- **Temperature**: the effective (black-body) temperature for the solid
  bodies. For the polytropes, the profile below.
- **Albedo**: three bands, long to short wavelength, in the order of
  `gx_core::radiance::BAND_EDGES`. Each body's values are its geometric
  albedo tinted toward its visible color, with the band mean equal to the
  geometric albedo (`docs/data-sources.md`, "Albedo bands"). The Sun's is 0,
  so it emits as a blackbody.
- **Roughness**: a model choice between 0 and 1: 0.9 for bare regolith
  (Mercury, the Moon), 0.85 for Mars, 0.6 for the Earth, 0.5 for Venus's
  cloud deck, 0.4 for the giant planets' cloud tops, 1.0 for the Sun.
- **Attenuation**: the mass attenuation coefficient. It is 0 for solid
  bodies, whose surface is the boundary of the ball. For gas and plasma
  bodies it is derived, never stated in the data: the photosphere
  condition puts optical depth 2/3 (integrated from the surface through
  the polytropic density) at the level whose temperature is the body's
  published one: the effective temperature for the plasma body, the 1 bar
  level for the gas bodies (below). Measured: 0.807 m^2/kg for the Sun,
  1.7e-4 for Jupiter, 7.5e-5 for Saturn and Neptune, 6.0e-5 for Uranus.
  Each is opaque within one percent of its radius (a test asserts optical
  depth 10 or more there).
- **Radiating temperature**: what the sample shows to the outside
  (`matter-format.md` section 3.3), below.

`System::density_at` and `System::sample_at` evaluate this model at a point
in a frame; the chunk compiler samples them into cells (`docs/compile.md`).

### Polytropes

A polytrope of index `n` has pressure `P = K rho^(1 + 1/n)`. In hydrostatic
equilibrium its density is `rho = rho_c theta^n`, where `theta(xi)` solves
the Lane-Emden equation

```text
theta'' + (2 / xi) theta' + theta^n = 0,   theta(0) = 1, theta'(0) = 0,
```

and `r = alpha xi`. The surface is the first zero `xi_1`, so `alpha = R /
xi_1` for mean radius `R`, and matching the mass fixes the central density
at `rho_c = rho_mean xi_1 / (3 |theta'(xi_1)|)`: 3.290 times the mean for
index 1 and 54.18 times for index 3.

`polytrope::LaneEmden::solve` integrates the equation with a fixed
deterministic scheme: the series `1 - xi^2/6 + n xi^4/120` for the first
step off the singular center, classical fourth order Runge-Kutta at a fixed
step of 1/2048 in `xi` until `theta` changes sign, and 64 bisections of the
cubic Hermite interpolant of the last step for `xi_1`. For index 1 the
result matches the closed form `sin(xi) / xi` (`xi_1 = pi`); for index 3 it
matches the published `xi_1 = 6.89685` and `rho_c / rho_mean = 54.1825`
(unit tests in `src/polytrope.rs`). `polytrope::Profile` tabulates
`theta^n` and `theta^n T` at 4097 points uniform in `s = (r / R)^2`, so a
sample point needs no square root, and reads them by linear interpolation.

Measured (`tests/compile.rs`): the summed mass of each polytrope at depth 0,
where one cell covers the whole body, is within 6.1e-7 of its declared mass
(the brief allows 2e-2), and within 6.9e-6 at depths 1 and 2. The density
at the outer edge, at 0.999 R, is below 1 percent of the mean.

### Temperature

The ideal-gas polytropic relation is `T ~ P / rho ~ theta`. It is scaled to
one published temperature per body:

- **Plasma body (index 3)**: `T = max(T_c theta, T_atm)`, with the
  published central temperature `T_c = 1.571e7 K` and the gray Eddington
  atmosphere `T_atm^4 = (3/4) T_eff^4 (tau + 2/3)` of the effective
  temperature `T_eff = 5772 K`, `tau` the optical depth from the surface.
  The photosphere condition (above) makes the two meet at `tau = 2/3`,
  where both are `T_eff`: the interior below, the atmosphere above, falling
  to `T_eff (1/2)^(1/4) = 4854 K` at the surface. Radiation leaving such an
  atmosphere carries `sigma T_eff^4`; in the three visible bands it is 1.2
  times `pi B(T_eff)` (`tests/light.rs`), because the deeper layers it
  shows are hotter.
- **Gas bodies (index 1)**: the published temperature at the 1 bar level is
  held at that level of the model. For index 1, `K = 2 pi G alpha^2`, so the
  model's own pressure `P = K rho_c^2 theta^2` gives the `theta_ref` where
  `P = 1e5 Pa`. Then `T = T_1bar max(1, theta / theta_ref)`: the polytropic
  relation scaled to pass through the published value at the model's 1 bar
  level, and constant above it.

A 1 bar reference is used for the gas bodies because their 1 bar
temperatures are the published, measured values; their central
temperatures are model results with large spreads. The central temperature
is used for the plasma body because that is its published value and its
1 bar level is not meaningful. Each sample's temperature is the
density-weighted mean over its sub-sample points.

### Radiating temperature

A sample's `temperature` is the mean of everything it holds; light leaves
only from about one optical depth below its outer side. A coarse sample at
the edge of the Sun holds matter at hundreds of thousands of kelvin but
shows only its photosphere. So each sample also carries the radiating
temperature (`src/polytrope.rs`, "Radiating temperature"). A sample
radiates the same in every direction, so it must carry the mean radiance of
the outward flux, `G(r) = F(r) / pi`, not the brighter radiance straight up
the radius (the disc center, without limb darkening). `G` is tabulated
from the depth of optical depth 60 to the surface by the formal solution of
the transfer equation along 32 directions, `G = 2 integral mu I(mu) dmu`;
a sample spanning radii `r_in` to `r_out` (its sub-cube's nearest and
farthest points from the center) with optical thickness `dtau` sends out
`(G(r_out) - e^-dtau G(r_in)) / (1 - e^-dtau)`, and its radiating
temperature is the blackbody temperature of that radiance summed over the
three bands. Uniform bodies have it equal to their temperature.

Measured through the library's emission law on the depth-0 sections
(`tests/light.rs`): the Sun sends out 1.45 times `4 pi R^2 pi B(T_eff)` in
the visible bands, the gray atmosphere's 1.2 times the depth-0 grid's
coarse outline (eight samples across the ball cover more than its disc);
a whole Sun section at 16, 32, and 64 samples across gives 1.35, 1.29, and
1.25. The 1.2 is the gray Eddington atmosphere's: its deeper, hotter
layers show more in the visible bands than `T_eff^4` says, where the real
Sun's line blanketing takes some back. No planet sends out any light at
1000 K or above.

## Deliberately not modeled

- **Solid interiors.** Solid bodies have one mean density from center to
  surface. Their mass and outer radius are right. The moments in the
  registry come from the published factors, not from the matter.
- **Oblateness in the matter.** Every body is a ball of mean radius. The
  giant planets' flattening appears in the registry's `j2` and moments,
  which the integrator uses, but not in the samples.
- **Atmospheres as separate layers.** The giant planets are gas throughout;
  the rocky bodies end at their mean radius.
- **Rings.** Not modeled.
- **Surface detail and fluids.** Topography belongs to the terrain module
  (`space-model.md` section 11 item 2), which owns the third planet's frame
  matter.
- **Satellites other than the Moon**, and the mass they add to their
  planets' systems.
