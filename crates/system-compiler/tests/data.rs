//! Checks of the baked data in `data/system.toml`: the registry at the data
//! epoch, the masses, the orbits implied by the state vectors, the
//! oblateness and moments, the rotation, and the matter model.

mod common;

use common::{system, DAY, EARTH, GAS, MOON, PLANETS, SUN};
use gx_core::frames::FrameSystem;
use gx_core::matter::{Sample, State};
use gx_core::registry::{self, FrameTree, ROOT_PARENT};
use gx_core::units::{Seconds, Vec3};
use system_compiler::orbit::osculating;
use system_compiler::{Structure, System};

const J2000: Seconds = Seconds::new(0.0);

#[test]
fn eleven_frames_with_the_expected_tree() {
    let s = system();
    let r = s.epoch_registry();
    let ids: Vec<u64> = r.frames().iter().map(|f| f.frame_id).collect();
    assert_eq!(ids, (0..=10).collect::<Vec<u64>>());
    for f in r.frames() {
        let expected_parent = match f.frame_id {
            0 => ROOT_PARENT,
            MOON => EARTH,
            _ => 0,
        };
        assert_eq!(f.parent_frame_id, expected_parent, "frame {}", f.frame_id);
    }
    assert_eq!(r.epoch().value(), 0.0);
}

#[test]
fn registry_round_trips_and_forms_a_tree() {
    let r = system().epoch_registry().clone();
    let bytes = registry::encode(&r);
    registry::validate(&bytes).expect("encoded registry validates");
    let decoded = registry::decode(&bytes).expect("encoded registry decodes");
    assert_eq!(decoded, r);
    let tree = FrameTree::from_registries(&[decoded]).expect("one registry forms a tree");
    assert_eq!(tree.root().frame_id, 0);
    assert_eq!(tree.children(EARTH), &[MOON]);
}

#[test]
fn bundled_data_is_the_repository_file() {
    assert_eq!(system(), System::bundled());
}

#[test]
fn root_frame() {
    let s = system();
    let root = s.epoch_registry().frames()[0];
    assert_eq!(root.mass.value(), 0.0);
    assert_eq!(root.root_extent.value(), 2.0e13);
    assert_eq!(root.max_depth, 0);
    assert_eq!(root.j2, 0.0);
    assert_eq!(root.principal_moments, Vec3::zero());
    let sun = s.body(SUN).unwrap();
    for id in PLANETS {
        let p = s.body(id).unwrap();
        let orbit = osculating(
            p.position - sun.position,
            p.velocity - sun.velocity,
            sun.mass,
        )
        .expect("bound orbit");
        assert!(
            orbit.apoapsis.value() < 0.5 * root.root_extent.value(),
            "{}",
            p.name
        );
    }
}

#[test]
fn body_frame_extents_and_depths() {
    let s = system();
    for b in s.bodies() {
        let expected = 8.0 * b.mean_radius.value();
        assert!(
            (b.root_extent.value() / expected - 1.0).abs() < 1e-15,
            "{}",
            b.name
        );
        assert_eq!(b.max_depth, 6, "{}", b.name);
    }
}

#[test]
fn sun_mass() {
    let m = system().body(SUN).unwrap().mass.value();
    assert!((m / 1.98847e30 - 1.0).abs() < 1e-3, "sun mass {m:e}");
}

#[test]
fn planet_periods_from_vis_viva_match_published() {
    let s = system();
    let sun = s.body(SUN).unwrap();
    for id in PLANETS {
        let p = s.body(id).unwrap();
        let orbit = osculating(
            p.position - sun.position,
            p.velocity - sun.velocity,
            sun.mass,
        )
        .expect("bound orbit");
        let published = p.sidereal_orbit_period.expect("published period").value();
        let error = (orbit.period.value() / published - 1.0).abs();
        assert!(
            error < 0.01,
            "{}: derived {:.3} d, published {:.3} d",
            p.name,
            orbit.period.value() / DAY,
            published / DAY
        );
    }
}

#[test]
fn moon_period_about_earth() {
    let s = system();
    let earth = s.body(EARTH).unwrap();
    let moon = s.body(MOON).unwrap();
    let orbit = osculating(moon.position, moon.velocity, earth.mass + moon.mass).unwrap();
    let days = orbit.period.value() / DAY;
    let published = moon.sidereal_orbit_period.unwrap().value() / DAY;
    assert!(
        (days / published - 1.0).abs() < 0.02,
        "moon period {days} d"
    );
}

#[test]
fn oblateness_and_moments_are_published_factors() {
    // C / (M R^2) as quoted in each body's comment, and J2 = (C - A) /
    // (M R_ref^2) for an axisymmetric body.
    let factors = [
        (1, 0.070),
        (2, 0.35),
        (3, 0.337),
        (4, 0.3308),
        (5, 0.366),
        (6, 0.254),
        (7, 0.210),
        (8, 0.225),
        (9, 0.23),
        (10, 0.394),
    ];
    let s = system();
    for (id, f) in factors {
        let b = s.body(id).unwrap();
        let m = b.mass.value();
        let r = b.mean_radius.value();
        let rr = b.reference_radius.value();
        let p = b.principal_moments;
        assert!(b.j2 > 0.0 && rr > 0.0, "{}", b.name);
        assert_eq!(p.x, p.y, "{}", b.name);
        assert!((p.z / (m * r * r) / f - 1.0).abs() < 1e-12, "{}", b.name);
        let j2 = (p.z - p.x) / (m * rr * rr);
        assert!((j2 / b.j2 - 1.0).abs() < 1e-6, "{}: {j2:e}", b.name);
        // The reference radius is an equatorial radius, near the mean (the
        // second planet's is 0.04 km below it).
        assert!(rr > 0.9999 * r && rr < 1.07 * r, "{}", b.name);
    }
    // The registry carries them.
    for f in s.epoch_registry().frames().iter().skip(1) {
        let b = s.body(f.frame_id).unwrap();
        assert_eq!(f.j2, b.j2);
        assert_eq!(f.reference_radius, b.reference_radius);
        assert_eq!(f.principal_moments, b.principal_moments);
    }
}

#[test]
fn iau_elements_at_j2000_match_the_values_baked_before() {
    // The values the previous data file held: the same pck00011.tpc
    // elements evaluated at J2000 with every periodic term included.
    let before = [
        (1, 286.13, 63.87, 84.176, 14.1844),
        (2, 281.0103, 61.4155, 329.599948805, 6.1385108),
        (3, 272.76, 67.16, 160.2, -1.4813688),
        (4, 0.0, 90.0, 190.147, 360.9856235),
        (
            5,
            317.680854407,
            52.886439275,
            176.632059732,
            350.8919824433,
        ),
        (6, 268.057204043, 64.495809953, 284.95, 870.536),
        (7, 40.589, 83.537, 38.9, 810.7939024),
        (8, 257.311, -15.175, 203.81, -501.1600928),
        (9, 299.333738959, 42.950359022, 249.996007571, 541.1397757),
        (10, 266.857733445, 65.641102748, 41.195263981, 13.17635815),
    ];
    let s = system();
    for (id, ra, dec, w, rate) in before {
        let a = s.body(id).unwrap().rotation.at(J2000);
        assert!(
            (a.pole_ra_deg - ra).abs() < 1e-8,
            "{id}: ra {}",
            a.pole_ra_deg
        );
        assert!(
            (a.pole_dec_deg - dec).abs() < 1e-8,
            "{id}: dec {}",
            a.pole_dec_deg
        );
        assert!(
            (a.prime_meridian_deg - w).abs() < 1e-8,
            "{id}: w {}",
            a.prime_meridian_deg
        );
        // The rate now includes the periodic terms' derivatives. The largest
        // is the innermost planet's 88 day libration, 0.012 percent of its
        // rate.
        assert!(
            (a.rate_deg_per_day / rate - 1.0).abs() < 2e-4,
            "{id}: rate {}",
            a.rate_deg_per_day
        );
    }
}

#[test]
fn earth_sidereal_day() {
    let f = system().epoch_registry().frames()[EARTH as usize];
    let day = 2.0 * std::f64::consts::PI / f.angular_velocity.length();
    assert!((day / 86164.1 - 1.0).abs() < 1e-4, "sidereal day {day} s");
}

#[test]
fn body_z_axis_points_at_the_iau_pole() {
    let s = system();
    let tree = FrameTree::from_registries(std::slice::from_ref(s.epoch_registry())).unwrap();
    let frames = FrameSystem::from_tree(tree);
    for b in s.bodies() {
        let a = b.rotation.at(J2000);
        let (ra, dec) = (a.pole_ra_deg.to_radians(), a.pole_dec_deg.to_radians());
        let pole = Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin());
        let z = frames
            .root_orientation(b.frame_id)
            .rotate(Vec3::new(0.0, 0.0, 1.0));
        assert!((z - pole).length() < 1e-9, "{}", b.name);
        let w = frames.state(b.frame_id).unwrap().angular_velocity;
        assert_eq!((w.x, w.y), (0.0, 0.0));
        assert_eq!(w.z, a.rate().value());
    }
}

#[test]
fn moon_orientation_is_relative_to_earth_axes() {
    let s = system();
    let earth = s.body(EARTH).unwrap().rotation.at(J2000).orientation();
    let moon_icrf = s.body(MOON).unwrap().rotation.at(J2000).orientation();
    let rel = s.orientation_at(MOON, J2000);
    let v = Vec3::new(0.3, -0.5, 0.8);
    assert!(((earth * rel).rotate(v) - moon_icrf.rotate(v)).length() < 1e-15);
}

#[test]
fn retrograde_rotators_have_negative_rate() {
    let s = system();
    for b in s.bodies() {
        let retro = matches!(b.name.as_str(), "Venus" | "Uranus");
        assert_eq!(
            b.rotation.at(J2000).rate().value() < 0.0,
            retro,
            "{}",
            b.name
        );
    }
}

#[test]
fn states_and_structures_match_the_brief() {
    let s = system();
    for b in s.bodies() {
        let (state, index) = match b.frame_id {
            SUN => (State::Plasma, Some(3)),
            6..=9 => (State::Gas, Some(1)),
            _ => (State::Solid, None),
        };
        assert_eq!(b.matter.state, state, "{}", b.name);
        match (&b.structure, index) {
            (Structure::Uniform, None) => {}
            (Structure::Polytrope(p), Some(3)) => assert!((p.xi1() - 6.89685).abs() < 1e-5),
            (Structure::Polytrope(p), Some(1)) => {
                assert!((p.xi1() - std::f64::consts::PI).abs() < 1e-9)
            }
            _ => panic!("{}: wrong structure", b.name),
        }
    }
    let sun = s.body(SUN).unwrap();
    assert_eq!(sun.matter.temperature.value(), 5772.0);
    assert_eq!(sun.matter.central_temperature.unwrap().value(), 1.571e7);
}

#[test]
fn polytrope_temperatures() {
    let s = system();
    let sun = s.body(SUN).unwrap();
    let r = sun.mean_radius.value();
    // Central temperature at the center; at the edge the Eddington
    // atmosphere's T_eff (1/2)^(1/4), optical depth 0.
    assert!((sun.temperature_at_radius(0.0).value() / 1.571e7 - 1.0).abs() < 1e-9);
    let top = 5772.0 * 0.5f64.sqrt().sqrt();
    assert!((sun.temperature_at_radius(r).value() / top - 1.0).abs() < 1e-12);
    // The attenuation comes from the photosphere condition.
    let kappa = sun.matter.attenuation.value();
    assert!(kappa > 0.0);
    let t1bar = [(6, 165.0), (7, 134.0), (8, 76.0), (9, 72.0)];
    for (id, t) in t1bar {
        let b = s.body(id).unwrap();
        let r = b.mean_radius.value();
        assert_eq!(b.temperature_at_radius(r).value(), t, "{}", b.name);
        let Structure::Polytrope(p) = &b.structure else {
            panic!()
        };
        // The 1 bar level of an index 1 polytrope of this size lies in the
        // outer 0.1 percent of the radius; the interior is hotter.
        assert!(p.reference_radius() > 0.999 * r, "{}", b.name);
        assert!(
            b.temperature_at_radius(0.5 * r).value() > 10.0 * t,
            "{}",
            b.name
        );
    }
}

#[test]
fn gas_and_plasma_bodies_are_opaque_within_one_percent_of_the_radius() {
    let s = system();
    let mut checked = 0;
    for b in s.bodies() {
        // Optical depth of the outer one percent along a radius, through the
        // profile: midpoint rule on 10000 shells.
        let r = b.mean_radius.value();
        let shells = 10_000;
        let dr = 0.01 * r / shells as f64;
        let column: f64 = (0..shells)
            .map(|i| b.density_at_radius(r - (i as f64 + 0.5) * dr).value() * dr)
            .sum();
        let tau = b.matter.attenuation.value() * column;
        match b.matter.state {
            State::Gas | State::Plasma => {
                assert!(tau >= 10.0, "{}: optical depth {tau}", b.name);
                checked += 1;
            }
            _ => assert_eq!(b.matter.attenuation.value(), 0.0, "{}", b.name),
        }
    }
    assert_eq!(checked, 5);
}

#[test]
fn matter_model_by_structure() {
    let s = system();
    for b in s.bodies() {
        let rho = b.mean_density();
        let r = b.mean_radius.value();
        match b.structure {
            Structure::Uniform => {
                assert_eq!(s.density_at(b.frame_id, Vec3::zero()), rho);
                assert_eq!(
                    s.density_at(b.frame_id, Vec3::new(0.0, 0.999 * r, 0.0)),
                    rho
                );
            }
            Structure::Polytrope(_) => {
                // Dense core, thin outer layers.
                let center = s.density_at(b.frame_id, Vec3::zero()).value();
                assert!(center > 3.0 * rho.value(), "{}", b.name);
                let edge = s.density_at(b.frame_id, Vec3::new(0.0, 0.999 * r, 0.0));
                assert!(edge.value() < 0.01 * rho.value(), "{}", b.name);
            }
        }
        assert_eq!(
            s.density_at(b.frame_id, Vec3::new(0.0, 0.0, 1.001 * r))
                .value(),
            0.0
        );
        let inside = s.sample_at(b.frame_id, Vec3::new(0.5 * r, 0.0, 0.0));
        assert_eq!(inside.state, b.matter.state);
        assert_eq!(inside.albedo, b.matter.albedo);
        assert_eq!(
            s.sample_at(b.frame_id, Vec3::new(r, r, 0.0)),
            Sample::VACUUM
        );
    }
    assert_eq!(s.sample_at(0, Vec3::zero()), Sample::VACUUM);
    for id in GAS {
        assert!(matches!(
            s.body(id).unwrap().structure,
            Structure::Polytrope(_)
        ));
    }
}

#[test]
fn mean_densities_are_plausible() {
    // Published mean densities in kg/m^3 (NASA NSSDCA fact sheets).
    let s = system();
    let published = [
        (2, 5429.0),
        (3, 5243.0),
        (4, 5514.0),
        (5, 3934.0),
        (6, 1326.0),
        (7, 687.0),
        (8, 1270.0),
        (9, 1638.0),
        (10, 3344.0),
    ];
    for (id, rho) in published {
        let b = s.body(id).unwrap();
        let got = b.mean_density().value();
        assert!((got / rho - 1.0).abs() < 0.01, "{}: {got} vs {rho}", b.name);
    }
}
