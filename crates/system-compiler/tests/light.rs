//! The light the compiled matter sends into space, through the library's
//! radiative transfer (`gx_core::emission::summarize`) on each frame's
//! depth-0 section, the one the renderer lights with.

mod common;

use common::*;
use core::f64::consts::PI;
use gx_core::emission::summarize;
use gx_core::key::{CellGeometry, CellKey};
use gx_core::matter::Section;
use gx_core::radiance::band_radiance;
use gx_core::units::{Kelvin, Meters, Vec3};
use system_compiler::compile::{sample_cell, section, CompileOptions};

/// Visible power of a sphere of radius `r` radiating as a blackbody at
/// `t`: `4 pi r^2 * pi B`, summed over the three bands.
fn sphere_power(r: f64, t: f64) -> f64 {
    let b: f64 = band_radiance(Kelvin::new(t)).iter().sum();
    4.0 * PI * r * r * PI * b
}

#[test]
fn only_the_star_shines_and_about_as_bright_as_its_photosphere() {
    let s = system();
    let opts = CompileOptions::default();
    for b in s.bodies() {
        let key = CellKey::new(b.frame_id, 0, 0, 0, 0).unwrap();
        let e = summarize(&section(&s, &key, &opts).unwrap(), Kelvin::new(1000.0));
        if b.frame_id == SUN {
            let p: f64 = e.unwrap().band_power.iter().sum();
            let ratio = p / sphere_power(b.mean_radius.value(), 5772.0);
            // Measured 2.0: the gray atmosphere's own 1.2 in these bands
            // (below) times the depth-0 grid's coarse outline (8 samples
            // across the ball).
            assert!(ratio > 1.0 && ratio < 2.5, "ratio {ratio}");
        } else {
            assert_eq!(e, None, "{}", b.name);
        }
    }
}

#[test]
fn finer_grids_approach_the_atmosphere() {
    let s = system();
    let sun = s.body(SUN).unwrap();
    let r = sun.mean_radius.value();
    let mut last = f64::INFINITY;
    for n in [16u8, 32, 64] {
        let edge = 2.0002 * r;
        let g = CellGeometry {
            origin: Vec3::new(-edge / 2.0, -edge / 2.0, -edge / 2.0),
            edge: Meters::new(edge),
        };
        let key = CellKey::new(SUN, 0, 0, 0, 0).unwrap();
        let sec = Section::new(key, g.origin, g.edge, n, sample_cell(sun, &g, n)).unwrap();
        let p: f64 = summarize(&sec, Kelvin::new(1000.0))
            .unwrap()
            .band_power
            .iter()
            .sum();
        let ratio = p / sphere_power(r, 5772.0);
        assert!(ratio < last && ratio > 1.0, "n {n}: {ratio}");
        last = ratio;
    }
}

#[test]
fn the_atmosphere_carries_the_photosphere_flux() {
    // A fine radial column through the profile: the emergent flux, by the
    // exact plane-parallel formal solution F = 2 pi integral B E2(tau) dtau,
    // against pi B(T_eff). A gray Eddington atmosphere carries sigma T_eff^4
    // in total; in the three visible bands it gives a little more, because
    // the deeper, hotter layers it shows are brighter there than T^4 says.
    let s = system();
    let sun = s.body(SUN).unwrap();
    let r = sun.mean_radius.value();
    let kappa = sun.matter.attenuation.value();
    let two_e3 = |t: f64| -> f64 {
        let m = 2000;
        let mut acc = 0.0;
        for k in 0..m {
            let mu = (k as f64 + 0.5) / m as f64;
            acc += mu * (-t / mu).exp();
        }
        2.0 * acc / m as f64
    };
    let (dr, mut tau, mut f, mut d) = (1000.0, 0.0f64, 0.0, 0.0);
    while tau < 30.0 {
        let rr = r - d - 0.5 * dr;
        let dt = kappa * sun.density_at_radius(rr).value() * dr;
        let b: f64 = band_radiance(sun.temperature_at_radius(rr)).iter().sum();
        f += PI * b * (two_e3(tau) - two_e3(tau + dt));
        tau += dt;
        d += dr;
    }
    let want = PI * band_radiance(Kelvin::new(5772.0)).iter().sum::<f64>();
    let ratio = f / want;
    assert!(ratio > 1.0 && ratio < 1.3, "ratio {ratio}");
}
