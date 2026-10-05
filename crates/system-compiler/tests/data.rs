//! Checks of the baked data in `data/system.toml`: the registry, the masses,
//! the orbits implied by the state vectors, the rotation, and the matter
//! model. Tolerances are those of the task brief for this data.

mod common;

use common::{system, DAY, EARTH, MOON, PLANETS, SUN};
use gx_core::frames::FrameSystem;
use gx_core::matter::{Sample, State};
use gx_core::registry::{self, FrameTree, ROOT_PARENT};
use gx_core::units::Vec3;
use system_compiler::orbit::osculating;
use system_compiler::System;

#[test]
fn eleven_frames_with_the_expected_tree() {
    let s = system();
    let r = s.registry();
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
    let r = system().registry();
    let bytes = registry::encode(&r);
    registry::validate(&bytes).expect("encoded registry validates");
    let decoded = registry::decode(&bytes).expect("encoded registry decodes");
    assert_eq!(decoded, r);
    assert_eq!(registry::encode(&decoded), bytes);
    let tree = FrameTree::from_registries(&[decoded]).expect("one registry forms a tree");
    assert_eq!(tree.root().frame_id, 0);
    assert_eq!(tree.children(EARTH), &[MOON]);
}

#[test]
fn registry_bytes_are_deterministic() {
    let a = registry::encode(&system().registry());
    let b = registry::encode(&System::bundled().registry());
    assert_eq!(a, b);
}

#[test]
fn root_frame() {
    let s = system();
    let root = s.registry().frames()[0];
    assert_eq!(root.mass.value(), 0.0);
    assert_eq!(root.root_extent.value(), 2.0e13);
    assert_eq!(root.max_depth, 0);
    // The root cube contains every planet's aphelion.
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
            "{}: extent {} vs {}",
            b.name,
            b.root_extent.value(),
            expected
        );
        assert_eq!(b.max_depth, 6, "{}", b.name);
    }
}

#[test]
fn sun_mass() {
    let s = system();
    let m = s.body(SUN).unwrap().mass.value();
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
        let derived = orbit.period.value();
        let error = (derived / published - 1.0).abs();
        assert!(
            error < 0.01,
            "{}: derived {:.3} d, published {:.3} d, error {:.3} %",
            p.name,
            derived / DAY,
            published / DAY,
            error * 100.0
        );
    }
}

#[test]
fn moon_period_about_earth() {
    let s = system();
    let earth = s.body(EARTH).unwrap();
    let moon = s.body(MOON).unwrap();
    // The Moon's state is relative to the Earth already. Two-body problem:
    // the central mass is the sum of both.
    let orbit = osculating(moon.position, moon.velocity, earth.mass + moon.mass).unwrap();
    let days = orbit.period.value() / DAY;
    assert!((days / 27.32 - 1.0).abs() < 0.02, "moon period {days} d");
    let published = moon.sidereal_orbit_period.unwrap().value() / DAY;
    assert!((days / published - 1.0).abs() < 0.02);
}

#[test]
fn earth_sidereal_day() {
    let s = system();
    let f = s
        .registry()
        .frames()
        .iter()
        .find(|f| f.frame_id == EARTH)
        .copied()
        .unwrap();
    let w = f.angular_velocity.length();
    let day = 2.0 * std::f64::consts::PI / w;
    assert!((day / 86164.1 - 1.0).abs() < 1e-4, "sidereal day {day} s");
}

#[test]
fn body_z_axis_points_at_the_iau_pole() {
    let s = system();
    let frames = FrameSystem::from_tree(FrameTree::from_registries(&[s.registry()]).unwrap());
    for b in s.bodies() {
        let (ra, dec) = (
            b.rotation.pole_ra_deg.to_radians(),
            b.rotation.pole_dec_deg.to_radians(),
        );
        let pole = Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin());
        // Body axes relative to root (ICRF) axes, composed along the chain.
        let q = frames.root_orientation(b.frame_id);
        let z = q.rotate(Vec3::new(0.0, 0.0, 1.0));
        assert!(
            (z - pole).length() < 1e-9,
            "{}: z {z:?} pole {pole:?}",
            b.name
        );
        // Angular velocity is about body z only, at the IAU rate.
        let w = frames.state(b.frame_id).unwrap().angular_velocity;
        assert_eq!((w.x, w.y), (0.0, 0.0));
        assert_eq!(w.z, b.rotation.rate().value());
    }
}

#[test]
fn moon_orientation_is_relative_to_earth_axes() {
    let s = system();
    let earth = s.body(EARTH).unwrap().rotation.orientation();
    let moon_icrf = s.body(MOON).unwrap().rotation.orientation();
    let rel = s.orientation(MOON).unwrap();
    let v = Vec3::new(0.3, -0.5, 0.8);
    assert!(((earth * rel).rotate(v) - moon_icrf.rotate(v)).length() < 1e-15);
    // Planets are children of the root, whose axes are the ICRF axes.
    for id in PLANETS {
        let b = s.body(id).unwrap();
        assert_eq!(
            s.orientation(id).unwrap(),
            b.rotation.orientation().normalized().unwrap()
        );
    }
}

#[test]
fn retrograde_rotators_have_negative_rate() {
    let s = system();
    for b in s.bodies() {
        let retro = matches!(b.name.as_str(), "Venus" | "Uranus");
        assert_eq!(b.rotation.rate().value() < 0.0, retro, "{}", b.name);
    }
}

#[test]
fn gas_and_plasma_bodies_are_opaque_within_one_percent_of_the_radius() {
    let s = system();
    let mut checked = 0;
    for b in s.bodies() {
        let tau =
            b.matter.attenuation.value() * b.mean_density().value() * 0.01 * b.mean_radius.value();
        match b.matter.state {
            State::Gas | State::Plasma => {
                assert!(
                    tau >= 10.0,
                    "{}: optical depth {tau} over 1 % of the radius",
                    b.name
                );
                checked += 1;
            }
            _ => assert_eq!(b.matter.attenuation.value(), 0.0, "{}", b.name),
        }
    }
    assert_eq!(checked, 5);
}

#[test]
fn states_match_the_brief() {
    let s = system();
    for b in s.bodies() {
        let expected = match b.frame_id {
            SUN => State::Plasma,
            6..=9 => State::Gas,
            _ => State::Solid,
        };
        assert_eq!(b.matter.state, expected, "{}", b.name);
    }
    assert_eq!(s.body(SUN).unwrap().matter.temperature.value(), 5772.0);
}

#[test]
fn matter_model_is_the_mean_density_ball() {
    let s = system();
    for b in s.bodies() {
        let rho = b.mean_density();
        assert!(rho.value() > 0.0);
        // Density times volume gives the mass back.
        let m = rho * b.volume();
        assert!(
            (m.value() / b.mass.value() - 1.0).abs() < 1e-14,
            "{}",
            b.name
        );
        let r = b.mean_radius.value();
        assert_eq!(s.density_at(b.frame_id, Vec3::zero()), rho);
        assert_eq!(
            s.density_at(b.frame_id, Vec3::new(0.0, 0.999 * r, 0.0)),
            rho
        );
        assert_eq!(
            s.density_at(b.frame_id, Vec3::new(0.0, 0.0, 1.001 * r))
                .value(),
            0.0
        );
        let inside = s.sample_at(b.frame_id, Vec3::new(0.5 * r, 0.0, 0.0));
        assert_eq!(inside.density, rho);
        assert_eq!(inside.state, b.matter.state);
        assert_eq!(inside.albedo, b.matter.albedo);
        assert_eq!(
            s.sample_at(b.frame_id, Vec3::new(r, r, 0.0)),
            Sample::VACUUM
        );
        for a in b.matter.albedo {
            assert!(a.is_in_unit_interval());
        }
    }
    // The root frame holds no matter.
    assert_eq!(s.sample_at(0, Vec3::zero()), Sample::VACUUM);
}

#[test]
fn mean_densities_are_plausible() {
    // Published mean densities in kg/m^3 (NASA NSSDCA fact sheets); the
    // model's mass over volume should agree within 1 percent.
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
