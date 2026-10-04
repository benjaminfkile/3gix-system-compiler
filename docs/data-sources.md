# Data sources

Every number in `data/system.toml` comes from one of the sources below. This
file records the exact requests, the response blocks the numbers were taken
from, and every conversion. All responses were fetched on 2026-10-04.

## Summary

| Quantity | Source | Conversion |
|---|---|---|
| `position_m`, `velocity_m_per_s` | JPL Horizons API, geometric state vectors (DE441) | km and km/s times 1000, digits kept exactly |
| `mass_kg` | NAIF kernel `gm_de440.tpc`, GM in km^3/s^2 (DE440 values) | `GM * 1e9 / 6.67430e-11` |
| `mean_radius_m` | Horizons physical data block, volumetric mean radius in km | times 1000 |
| `sidereal_orbit_period_s` (check value) | Horizons physical data block, sidereal orbit period in days | times 86400 |
| `pole_ra_deg`, `pole_dec_deg`, `prime_meridian_deg`, `rotation_rate_deg_per_day` | IAU WGCCRE report as encoded in NAIF kernel `pck00011.tpc` | evaluated at J2000 with every periodic term, see below |
| `temperature_k` | NASA NSSDCA fact sheets, black-body temperature; Horizons for the Sun's effective temperature | none |
| `albedo` | NASA NSSDCA fact sheets, geometric albedo, tinted per band | see "Albedo bands" |
| `roughness`, `attenuation_m2_per_kg` | chosen model parameters, see `docs/model.md` | none |
| `root_extent_m`, `max_depth` | chosen frame layout | `8 * mean_radius_m`; 6 |

`G = 6.67430e-11 m^3 kg^-1 s^-2` (CODATA 2018) is the value of
`gx_core::gravity::G`, so the masses produce exactly the published GM in the
integrator.

## State vectors: JPL Horizons

API endpoint `https://ssd.jpl.nasa.gov/api/horizons.api`, one request per
body with:

- `EPHEM_TYPE=VECTORS`, `VEC_TABLE=2` (position and velocity),
- `REF_PLANE=FRAME` (ICRF axes, the axes of every registry state vector),
- `OUT_UNITS=KM-S`,
- `TLIST='2451545.0'` (2000-01-01 12:00:00 TDB, the epoch,
  `tdb_seconds_since_j2000 = 0.0`),
- `CENTER='@0'` (the solar system barycenter, the root frame) for the Sun
  and the planets, and `CENTER='@399'` (the Earth's center) for the Moon,
  whose parent frame is the Earth,
- `OBJ_DATA=YES`, so the same response carries the physical data block the
  radius, period, and Sun temperature come from.

Targets are body centers (`10`, `199`, `299`, `399`, `499`, `599`, `699`,
`799`, `899`, `301`), not system barycenters, because each frame is the
center of a body's matter. Horizons computes the giant planets' centers from
their satellite ephemerides (named in the `Target body name` line).

Conversion: each value is multiplied by 1000 in decimal arithmetic, so the
digits in `data/system.toml` are exactly the digits Horizons returned with
the exponent raised by 3 (trailing zeros dropped).

### Sun (frame 1)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%2710%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Sun (10)                        {source: DE441}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X =-1.067706805380953E+06 Y =-3.960361847959462E+05 Z =-1.380651842868809E+05
 VX= 9.312571926520472E-03 VY=-1.170150612817771E-02 VZ=-5.251266205200356E-03
$$EOE
```

Physical data lines used (same response):

```text
  Vol. mean radius, km  = 695700              Volume, 10^12 km^3    = 1412000
  Mass-energy conv rate = 4.260 x 10^9 kg/s   Effective temp, K     = 5772
```

### Mercury (frame 2)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27199%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Mercury (199)                   {source: DE441}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X =-2.052943316123468E+07 Y =-6.032400395827633E+07 Z =-3.013083786411830E+07
 VX= 3.700430442920571E+01 VY=-8.541376789510446E+00 VZ=-8.398372409672424E+00
$$EOE
```

Physical data lines used (same response):

```text
  Vol. Mean Radius (km) =  2439.4+-0.1    Density (g cm^-3)     = 5.427
  Sidereal orb. per.    = 87.969257  d    Escape vel. km/s      =  4.435
```

### Venus (frame 3)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27299%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Venus (299)                     {source: DE441}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X =-1.085242008575715E+08 Y =-7.318564959678600E+06 Z = 3.548121861333776E+06
 VX= 1.391218601189967E+00 VY=-3.202951993781091E+01 VZ=-1.449708673947320E+01
$$EOE
```

Physical data lines used (same response):

```text
  Vol. Mean Radius (km) =  6051.84+-0.01 Density (g/cm^3)      =  5.204
  Sidereal orb. per., d = 224.70079922   Escape speed, km/s    =   10.361
```

### Earth (frame 4)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27399%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Earth (399)                     {source: DE441}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X =-2.756674048281145E+07 Y = 1.323613811535491E+08 Z = 5.741865328625385E+07
 VX=-2.978494749851088E+01 VY=-5.029753814928081E+00 VZ=-2.180645069035755E+00
$$EOE
```

Physical data lines used (same response):

```text
  Vol. Mean Radius (km)    = 6371.01+-0.02   Mass x10^24 (kg)= 5.97219+-0.0006
  Orbital speed, km/s      = 29.79       Sidereal orb period  = 365.25636 d
```

### Mars (frame 5)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27499%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Mars (499)                      {source: mar099}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X = 2.069804338364610E+08 Y =-1.864170112571357E+05 Z =-5.667227497442207E+06
 VX= 1.171984975692608E+00 VY= 2.390670819298843E+01 VZ= 1.093392065056128E+01
$$EOE
```

Physical data lines used (same response):

```text
  Vol. mean radius (km) = 3389.92+-0.04   Density (g/cm^3)      =  3.933(5+-4)
  Mean sidereal orb per =  686.98 d       Orbital speed,  km/s  =  24.13
```

### Jupiter (frame 6)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27599%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Jupiter (599)                   {source: jup365_merged}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X = 5.974999178516835E+08 Y = 4.089902697993661E+08 Z = 1.607562616932818E+08
 VX=-7.900547720245487E+00 VY= 1.017187257622670E+01 VZ= 4.552504127783227E+00
$$EOE
```

Physical data lines used (same response):

```text
  Vol. Mean Radius (km) = 69911+-6          Flattening            = 0.06487
  Sidereal orbit period = 11.861982204 y    Sidereal orbit period = 4332.589 d
```

### Saturn (frame 7)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27699%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Saturn (699)                    {source: sat441l}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X = 9.573176521103407E+08 Y = 9.233194350574769E+08 Z = 3.401627932740891E+08
 VX=-7.421900386838120E+00 VY= 6.098450820882325E+00 VZ= 2.837547973276323E+00
$$EOE
```

Physical data lines used (same response):

```text
  Vol. Mean Radius (km) = 58232+-6        Flattening             =  0.09796
  Sidereal orbit period = 29.447498 yr    Sidereal orbit period  = 10755.698 d
```

### Uranus (frame 8)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27799%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Uranus (799)                    {source: ura184_merged}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X = 2.157907112723417E+09 Y =-1.871307099571182E+09 Z =-8.501069259961469E+08
 VX= 4.646584677611653E+00 VY= 4.251110198227456E+00 VZ= 1.796121552064855E+00
$$EOE
```

Physical data lines used (same response):

```text
  Vol. Mean Radius (km) = 25362+-12       Flattening             =  0.02293
  Sidereal orbit period = 84.0120465 y    Sidereal orbit period  = 30685.4 d
```

### Neptune (frame 9)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27899%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@0%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Neptune (899)                   {source: nep098_merged}
Center body name: Solar System Barycenter (0)     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X = 2.513978816984908E+09 Y =-3.438170168305322E+09 Z =-1.469851595078865E+09
 VX= 4.474587918382480E+00 VY= 2.876584787253288E+00 VZ= 1.065773120376900E+00
$$EOE
```

Physical data lines used (same response):

```text
  Vol. mean radius (km) = 24624+-21       Polar radius (km)      = 24342+-30
  Sidereal orbit period = 164.788501027 y Sidereal orbit period  = 60189 d
```

### Moon (frame 10)

Request:

```text
https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND=%27301%27&OBJ_DATA=YES&MAKE_EPHEM=YES&EPHEM_TYPE=VECTORS&CENTER=%27@399%27&REF_PLANE=FRAME&OUT_UNITS=KM-S&VEC_TABLE=2&TLIST=%272451545.0%27
```

Response block:

```text
Target body name: Moon (301)                      {source: DE441}
Center body name: Earth (399)                     {source: DE441}
Start time      : A.D. 2000-Jan-01 12:00:00.0000 TDB
Output units    : KM-S
Reference frame : ICRF
$$SOE
2451545.000000000 = A.D. 2000-Jan-01 12:00:00.0000 TDB 
 X =-2.916083841877129E+05 Y =-2.667168338540655E+05 Z =-7.610248730658794E+04
 VX= 6.435313889889519E-01 VY=-6.660876829565195E-01 VZ=-3.013257046610932E-01
$$EOE
```

Physical data lines used (same response):

```text
  Vol. mean radius, km  = 1737.53+-0.03    Mass, x10^22 kg       =    7.349
  Obliquity to orbit    = 6.67 deg         Orbit period          = 27.321582 d
```

## Masses: NAIF `gm_de440.tpc`

URL: `https://naif.jpl.nasa.gov/pub/naif/generic_kernels/pck/gm_de440.tpc`.
GM values for the Sun, the planets' bodies (not their systems), and the
Moon, km^3/s^2:

```text
BODY10_GM      = ( 1.3271244004127942E+11 )
BODY199_GM     = ( 2.2031868551400003D+04 )
BODY299_GM     = ( 3.2485859200000000D+05 )
BODY301_GM     = ( 4.9028001184575496D+03 )
BODY399_GM     = ( 3.9860043550702266D+05 )
BODY499_GM     = ( 4.282837362069909E+04  )
BODY599_GM     = ( 1.266865319003704E+08  )
BODY699_GM     = ( 3.793120623436167E+07  )
BODY799_GM     = ( 5.793951256527211E+06  )
BODY899_GM     = ( 6.835103145462294E+06  )
```

`mass_kg = GM * 1e9 / 6.67430e-11`, computed in IEEE double precision and
written with the shortest decimal that round trips. Each value is also
quoted next to its mass in `data/system.toml`. The Sun's mass is
1.98841e30 kg, 0.003 percent from the IAU nominal 1.98847e30 kg.

The planet masses exclude their satellites. Only the Moon is modeled as a
satellite in v1, so the giant planets are each lighter than their systems by
at most a few parts in ten thousand.

## Rotation: IAU rotational elements

Source: the IAU Working Group on Cartographic Coordinates and Rotational
Elements (WGCCRE) report of 2015 (Archinal et al., Celestial Mechanics and
Dynamical Astronomy 130:22, 2018), as encoded by NAIF in the text kernel
`pck00011.tpc`, URL
`https://naif.jpl.nasa.gov/pub/naif/generic_kernels/pck/pck00011.tpc`. The
2015 report does not revise the Earth's or the Moon's orientation; the kernel
carries the 2009 report's values for those two, and they are used here.

Kernel lines (the polynomial coefficients are constant, per century or per
day for `T`, and per day squared):

```text
BODY599_POLE_RA        = (   268.056595     -0.006499       0. )
BODY599_POLE_DEC       = (    64.495303      0.002413       0. )
BODY599_PM             = (   284.95        870.5360000      0. )
BODY10_POLE_RA         = (  286.13       0.          0. )
BODY10_POLE_DEC        = (   63.87       0.          0. )
BODY10_PM              = (   84.176     14.18440     0. )
BODY199_POLE_RA          = (  281.0103   -0.0328     0. )
BODY199_POLE_DEC         = (   61.4155   -0.0049     0. )
BODY199_PM               = (  329.5988    6.1385108  0. )
BODY299_POLE_RA          = (  272.76       0.          0. )
BODY299_POLE_DEC         = (   67.16       0.          0. )
BODY299_PM               = (  160.20      -1.4813688   0. )
BODY399_POLE_RA        = (    0.      -0.641         0. )
BODY399_POLE_DEC       = (   90.      -0.557         0. )
BODY399_PM             = (  190.147  360.9856235     0. )
BODY499_POLE_RA          = (  317.269202  -0.10927547        0.  )
BODY499_POLE_DEC         = (   54.432516  -0.05827105        0.  )
BODY499_PM               = (  176.049863  +350.891982443297  0.  )
BODY699_POLE_RA        = (    40.589    -0.036      0.  )
BODY699_POLE_DEC       = (    83.537    -0.004      0.  )
BODY699_PM             = (    38.90    810.7939024  0.  )
BODY799_POLE_RA        = (  257.311     0.         0.  )
BODY799_POLE_DEC       = (  -15.175     0.         0.  )
BODY799_PM             = (  203.81   -501.1600928  0.  )
BODY899_POLE_RA        = (  299.36     0.         0. )
BODY899_POLE_DEC       = (   43.46     0.         0. )
BODY899_PM             = (  249.978  541.1397757  0. )
BODY301_POLE_RA      = (  269.9949        0.0031        0.      )
BODY301_POLE_DEC     = (   66.5392        0.0130        0.      )
BODY301_PM           = (   38.3213       13.17635815   -1.4D-12 )
```

For each body the elements are

```text
alpha0 = a0 + a1 T + sum_i ra_i sin(theta_i)
delta0 = d0 + d1 T + sum_i dec_i cos(theta_i)
W      = W0 + Wdot d + sum_i pm_i sin(theta_i)
```

with `d` days and `T` Julian centuries past J2000 TDB, and `theta_i` the
nutation precession angles of the body's system (`BODYn_NUT_PREC_ANGLES`,
coefficients `BODYnnn_NUT_PREC_RA`, `_DEC`, `_PM`). At the epoch `d = T =
0`, so every polynomial reduces to its constant term and every periodic term
to its coefficient times the sine or cosine of its angle's constant term.
`scripts/iau-at-epoch.py` evaluates this directly from the kernel:

```text
python3 scripts/iau-at-epoch.py pck00011.tpc
```

Output, the values in `data/system.toml`:

```text
body        pole_ra_deg   pole_dec_deg         W0_deg     Wdot_deg_day
Sun       286.130000000   63.870000000   84.176000000    14.1844000000
Mercury   281.010300000   61.415500000  329.599948805     6.1385108000
Venus     272.760000000   67.160000000  160.200000000    -1.4813688000
Earth       0.000000000   90.000000000  190.147000000   360.9856235000
Mars      317.680854407   52.886439275  176.632059732   350.8919824433
Jupiter   268.057204043   64.495809953  284.950000000   870.5360000000
Saturn     40.589000000   83.537000000   38.900000000   810.7939024000
Uranus    257.311000000  -15.175000000  203.810000000  -501.1600928000
Neptune   299.333738959   42.950359022  249.996007571   541.1397757000
Moon      266.857733445   65.641102748   41.195263981    13.1763581500
```

The periodic terms matter for the Moon (several degrees), Mars, Jupiter, and
Neptune; the other bodies have none at the pole or prime meridian.

The conversion to the registry (unit tests in
`crates/system-compiler/src/rotation.rs`):

- Body axes relative to ICRF:
  `q = Rz(alpha0 + 90 deg) * Rx(90 deg - delta0) * Rz(W)` as a unit
  quaternion. Body `z` is the north pole and body `x` points at the prime
  meridian.
- Orientation in the registry is relative to the parent's axes:
  `conj(q_parent) * q`. The root's axes are the ICRF axes, so planets carry
  `q` itself and the Moon carries `conj(q_earth) * q_moon`.
- Angular velocity `(0, 0, Wdot * pi / 180 / 86400)` rad/s about the body
  axes. `Wdot` is negative for retrograde rotation (Venus and Uranus).
- Sine and cosine come from `crates/system-compiler/src/detmath.rs`, written
  out with correctly rounded operations, so the registry bytes are the same
  on every machine.

## Temperature and albedo: NASA NSSDCA fact sheets

URLs `https://nssdc.gsfc.nasa.gov/planetary/factsheet/<name>fact.html` with
`<name>` one of `mercury`, `venus`, `earth`, `moon`, `mars`, `jupiter`,
`saturn`, `uranus`, `neptune`. Values used:

| Body | Black-body temperature (K) | Geometric albedo |
|---|---|---|
| Mercury | 439.6 | 0.142 |
| Venus | 226.6 | 0.689 |
| Earth | 254.0 | 0.434 |
| Moon | 270.4 | 0.12 |
| Mars | 209.8 | 0.170 |
| Jupiter | 109.9 | 0.538 |
| Saturn | 81.0 | 0.499 |
| Uranus | 58.1 | 0.488 |
| Neptune | 46.6 | 0.442 |

The black-body temperature is the effective (radiative equilibrium)
temperature. The Sun's 5772 K is the IAU 2015 nominal effective temperature,
quoted in the Horizons physical data block for target 10 above.

### Albedo bands

The matter format carries albedo in the three bands of
`gx_core::radiance::BAND_EDGES`, long to short wavelength: 600 to 700 nm, 500
to 600 nm, 400 to 500 nm. No single source gives band albedos for every
body, so each body's three values are its geometric albedo tinted toward its
visible color, with the mean of the three equal to the geometric albedo. The
tint for each body is stated in a comment next to its `albedo` in
`data/system.toml`. The Sun has albedo 0 in every band, so its emissivity
`1 - albedo` is 1 and it radiates as a blackbody at 5772 K.

## Published periods (check values)

`sidereal_orbit_period_s` comes from the Horizons physical data blocks above
(days times 86400). It is never compiled. The tests derive each planet's
period from its heliocentric state (planet minus Sun) with the vis-viva
equation and the Sun's mass, and the Moon's from its state relative to the
Earth with the Earth plus Moon mass, and require agreement within 1 percent
(planets) and 2 percent (Moon, against 27.32 days). Sources differ in the last
digits of some periods (Saturn: 10755.698 d in Horizons, 10755.699 d in the
NSSDCA fact sheet). Horizons is used throughout.
