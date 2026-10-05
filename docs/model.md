# How a body becomes matter

This document describes the model the system compiler uses to turn each body
in `data/system.toml` into a frame in the registry and into matter samples.
It implements `matter-format.md` sections 3 (matter sections) and 5 (frame
registry) and milestone 1 of `space-model.md` section 11. The code is in
`crates/system-compiler/src/model.rs`.

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
- `position`, `velocity`: the Horizons state relative to the parent at the
  epoch, in ICRF axes (the registry's root axes; translations add along the
  parent chain without rotation).
- `orientation`: the body's IAU axes (z on the north pole, x on the prime
  meridian) relative to the parent's axes. For the Moon that is relative to
  the Earth's axes.
- `angular_velocity`: `(0, 0, Wdot)` in radians per second about the body
  axes.
- `root_extent`: `8 * mean_radius`, a cube that contains the body with
  room; `max_depth`: 6.

The epoch is `tdb_seconds_since_j2000 = 0.0`, 2000-01-01 12:00:00 TDB.

## Matter

Every body is a ball of radius `mean_radius` (the radius of the sphere with
the body's volume) filled with one uniform material:

- **Density**: the mean density, `mass / (4/3 pi mean_radius^3)`, computed
  in code and never stored. Inside the ball (distance from the frame origin
  at most `mean_radius`) every sample has this density; outside it is
  vacuum. The mass of the ball is therefore exactly the frame's mass.
- **State**: `solid` for Mercury, Venus, Earth, Mars, and the Moon; `gas`
  for Jupiter, Saturn, Uranus, and Neptune; `plasma` for the Sun.
- **Temperature**: the effective (black-body) temperature for the planets
  and the Moon, 5772 K for the Sun. The renderer derives emission from it
  with Planck's law, scaled by emissivity `1 - albedo` per band.
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
  bodies it is chosen so the body is opaque within a small fraction of its
  radius: the optical depth over one percent of the radius at the mean
  density, `attenuation * density * 0.01 * radius`, is at least 10
  (transmittance below `e^-10`). The values are 1e-7 m^2/kg for the giant
  planets (optical depth 32 to 93) and 1e-8 m^2/kg for the Sun (98). A test
  asserts the inequality for every gas and plasma body.

`System::density_at` and `System::sample_at` evaluate this model at a point
in a frame; the chunk compiler samples them into cells.

## Deliberately not modeled in v1

- **Interior structure.** No cores, mantles, or density gradients: one mean
  density from center to surface. The mass and the outer radius are right;
  the moment of inertia is not.
- **Oblateness.** Every body is a ball of mean radius. The giant planets'
  flattening (up to about 10 percent for Saturn) and the Earth's are
  ignored, in both the matter and the gravity (the registry is point
  masses).
- **Atmospheres as separate layers.** An atmosphere is not a shell of
  different matter. The giant planets are gas throughout; the rocky bodies
  end at their mean radius.
- **Rings.** Not modeled.
- **Surface detail and fluids.** Topography and oceans belong to later
  compilers (`space-model.md` section 11, milestones 2 and 3), which add
  matter to the same frames.
- **Satellites other than the Moon**, and the mass they add to their
  planets' systems.
- **Precession and nutation.** Poles are fixed at their J2000 directions;
  the integrator turns each body at a constant angular velocity.

## Known limitation: the Moon's spin over time

The registry stores each frame's orientation relative to its parent's axes,
and gx-core's integrator advances each frame's orientation by its own
angular velocity relative to its parent and composes the chain. The Moon's
parent is the Earth, so as the simulation runs the Moon's axes are carried
around by the Earth's rotation as well as its own. At the epoch, which is
what the registry describes, the orientation is exact. Over time the Moon's
absolute spin is wrong by the Earth's rotation. This is a property of the
frame model in gx-core, not of the data here, and does not affect positions,
gravity, or any planet.
