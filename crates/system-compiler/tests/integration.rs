//! Integration sanity: the registry, gx-core's frame tree, and gx-core's
//! laws agree on real numbers. The whole system is integrated for 365.25
//! days from the epoch with `Yoshida4` and a 3600 s maximum step, checking
//! after every step that each planet stays between the perihelion and
//! aphelion of its osculating orbit at the epoch (with 1 percent slack) and
//! that the Moon stays between 300000 km and 410000 km from the Earth.

mod common;

use common::{system, DAY, EARTH, MOON, PLANETS, SUN};
use gx_core::frames::FrameSystem;
use gx_core::integrate::{advance, Scheme};
use gx_core::registry::FrameTree;
use gx_core::units::Seconds;
use system_compiler::orbit::osculating;

#[test]
fn one_year_of_orbits_stays_within_bounds() {
    let s = system();
    let sun = s.body(SUN).unwrap();
    let bounds: Vec<(u64, f64, f64)> = PLANETS
        .iter()
        .map(|&id| {
            let p = s.body(id).unwrap();
            let o = osculating(
                p.position - sun.position,
                p.velocity - sun.velocity,
                sun.mass,
            )
            .expect("bound orbit");
            (id, 0.99 * o.periapsis.value(), 1.01 * o.apoapsis.value())
        })
        .collect();

    let tree = FrameTree::from_registries(&[s.registry()]).unwrap();
    let mut frames = FrameSystem::from_tree(tree);
    let step = 3600.0;
    let end = 365.25 * DAY;
    let steps = (end / step) as u64;
    assert_eq!(steps as f64 * step, end);
    let (mut moon_min, mut moon_max) = (f64::INFINITY, 0.0_f64);
    for i in 1..=steps {
        let t = Seconds::new(i as f64 * step);
        advance(&mut frames, t, Seconds::new(step), Scheme::Yoshida4);
        let sun_at = frames.root_position(SUN);
        for &(id, lo, hi) in &bounds {
            let d = (frames.root_position(id) - sun_at).length();
            assert!(
                (lo..=hi).contains(&d),
                "frame {id} at day {:.2}: {d:e} m outside [{lo:e}, {hi:e}]",
                t.value() / DAY
            );
        }
        let moon = frames.state(MOON).unwrap().position.length();
        moon_min = moon_min.min(moon);
        moon_max = moon_max.max(moon);
        assert!(
            (3.0e8..=4.1e8).contains(&moon),
            "moon at day {:.2}: {moon:e} m from the earth",
            t.value() / DAY
        );
    }
    assert_eq!(frames.time().value(), end);
    // The Moon is stored relative to the Earth: check that against the
    // root positions too.
    let rel = frames.root_position(MOON) - frames.root_position(EARTH);
    assert!((rel - frames.state(MOON).unwrap().position).length() < 1.0);
    // A full year sweeps the Moon through perigee and apogee many times.
    assert!(
        moon_min < 3.7e8 && moon_max > 4.0e8,
        "{moon_min:e} {moon_max:e}"
    );
}
