//! The planetary system: parsed and validated data, the frame registry, and
//! the per-body matter model.
//!
//! [`System`] is built from `data/system.toml` (see `docs/data-sources.md`
//! for where every number comes from). Loading checks the data and builds
//! the frame registry once through gx-core's validated constructor, so a
//! loaded [`System`] always yields a registry that passes the rules of
//! `matter-format.md` section 5 and forms a single frame tree.
//!
//! Registry (`matter-format.md` section 5.2): one root frame for the system
//! barycenter, massless and at rest, and one frame per body with its mass,
//! position and velocity relative to its parent in ICRF axes at the epoch,
//! orientation relative to its parent's axes, and angular velocity about its
//! own axes.
//!
//! Matter (`matter-format.md` section 3.3, documented in `docs/model.md`):
//! each body is a ball of uniform mean density (mass over the volume of the
//! ball of mean radius) with one state, temperature, three-band albedo,
//! roughness, and attenuation. Outside the ball is vacuum. Every value is
//! carried in gx-core unit types.

use std::path::Path;
use std::str::FromStr;

use core::f64::consts::PI;

use gx_core::error::ValidationError;
use gx_core::matter::{Sample, State};
use gx_core::registry::{Frame, FrameTree, Registry, MAX_DEPTH, ROOT_PARENT};
use gx_core::units::{
    Attenuation, CubicMeters, Density, Kelvin, Kilograms, Meters, Quat, Ratio, Seconds, Vec3,
};
use serde::Deserialize;

use crate::rotation::RotationElements;

/// Why system data failed to load.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The data file could not be read.
    #[error("reading {path}: {source}")]
    Io {
        /// Path of the file.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The data is not valid TOML or does not match the schema.
    #[error("parsing system data: {0}")]
    Parse(#[from] toml::de::Error),
    /// The data has no `[root]` table.
    #[error("no [root] frame")]
    MissingRoot,
    /// Two frames share an id.
    #[error("frame_id {0} appears more than once")]
    DuplicateFrameId(u64),
    /// A body names a parent that is not declared.
    #[error("frame_id {frame_id}: parent_frame_id {parent} is not declared")]
    MissingParent {
        /// The body's frame id.
        frame_id: u64,
        /// The undeclared parent id.
        parent: u64,
    },
    /// A field breaks a rule.
    #[error("frame_id {frame_id}: {field} = {value:e} {rule}")]
    Invalid {
        /// The frame id the field belongs to.
        frame_id: u64,
        /// Field name as written in the data file.
        field: &'static str,
        /// The offending value.
        value: f64,
        /// The rule it breaks.
        rule: &'static str,
    },
    /// The epoch is not finite.
    #[error("epoch tdb_seconds_since_j2000 = {0} is not finite")]
    Epoch(f64),
    /// gx-core rejected the registry or the frame tree built from it.
    #[error("registry rejected: {0}")]
    Registry(#[from] ValidationError),
}

/// Result of loading system data.
pub type Result<T> = std::result::Result<T, ModelError>;

/// The root frame: the system barycenter, the origin of every state vector.
#[derive(Clone, Debug, PartialEq)]
pub struct RootFrame {
    /// Frame id of the root.
    pub frame_id: u64,
    /// Compiler-side name; never emitted.
    pub name: String,
    /// Mass of the root frame; 0 for a barycenter.
    pub mass: Kilograms,
    /// Edge of the cube of space the root frame's cells divide.
    pub root_extent: Meters,
    /// Deepest cell compiled in the root frame.
    pub max_depth: u8,
}

/// What a body is made of, uniformly, in v1 (`docs/model.md`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Matter {
    /// State of the matter; never vacuum.
    pub state: State,
    /// Temperature of the matter.
    pub temperature: Kelvin,
    /// Reflectance in the three bands of `gx_core::radiance::BAND_EDGES`,
    /// long to short wavelength.
    pub albedo: [Ratio; 3],
    /// Microfacet roughness.
    pub roughness: Ratio,
    /// Mass attenuation coefficient; 0 for a hard surface.
    pub attenuation: Attenuation,
}

/// One body: its frame, its state at the epoch, its rotation, and its matter.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// Frame id of the body's frame.
    pub frame_id: u64,
    /// Frame id of the parent frame.
    pub parent_frame_id: u64,
    /// Compiler-side name; never emitted.
    pub name: String,
    /// Gravitational mass.
    pub mass: Kilograms,
    /// Radius of the ball of the same volume as the body.
    pub mean_radius: Meters,
    /// Position relative to the parent at the epoch, meters, ICRF axes.
    pub position: Vec3,
    /// Velocity relative to the parent at the epoch, meters per second,
    /// ICRF axes.
    pub velocity: Vec3,
    /// Published sidereal orbital period about the parent, a check value
    /// that is never compiled.
    pub sidereal_orbit_period: Option<Seconds>,
    /// IAU rotational elements at the epoch.
    pub rotation: RotationElements,
    /// The uniform matter model.
    pub matter: Matter,
    /// Edge of the cube of space the body frame's cells divide.
    pub root_extent: Meters,
    /// Deepest cell compiled with non-empty matter in this frame.
    pub max_depth: u8,
}

impl Body {
    /// Volume of the ball of mean radius, `4/3 pi r^3`.
    pub fn volume(&self) -> CubicMeters {
        self.mean_radius.cubed() * (4.0 / 3.0 * PI)
    }

    /// Mean density: mass over [`Body::volume`].
    pub fn mean_density(&self) -> Density {
        Density::new(self.mass.value() / self.volume().value())
    }

    /// Returns `true` if a point in the body frame (meters from the body
    /// center) is inside the ball of mean radius, boundary included.
    pub fn contains(&self, p_in_frame: Vec3) -> bool {
        let r = self.mean_radius.value();
        p_in_frame.length_squared() <= r * r
    }

    /// The sample of the body's matter: mean density and the matter model.
    pub fn sample(&self) -> Sample {
        Sample {
            density: self.mean_density(),
            state: self.matter.state,
            temperature: self.matter.temperature,
            albedo: self.matter.albedo,
            roughness: self.matter.roughness,
            attenuation: self.matter.attenuation,
        }
    }
}

/// The parsed and validated system.
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    epoch: Seconds,
    root: RootFrame,
    /// Sorted by frame id ascending.
    bodies: Vec<Body>,
    registry: Registry,
}

impl System {
    /// Reads and validates a system data file.
    pub fn load(path: impl AsRef<Path>) -> Result<System> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ModelError::Io {
            path: path.display().to_string(),
            source,
        })?;
        text.parse()
    }

    /// The bundled `data/system.toml`, baked in at build time.
    pub fn bundled() -> System {
        crate::SYSTEM_TOML
            .parse()
            .expect("the bundled system data is valid")
    }

    /// Epoch of every state vector, seconds of TDB since J2000.
    pub fn epoch(&self) -> Seconds {
        self.epoch
    }

    /// The root frame.
    pub fn root(&self) -> &RootFrame {
        &self.root
    }

    /// Every body, sorted by frame id ascending.
    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    /// The body declaring `frame_id`, or `None` for the root or an unknown
    /// id.
    pub fn body(&self, frame_id: u64) -> Option<&Body> {
        self.bodies
            .binary_search_by_key(&frame_id, |b| b.frame_id)
            .ok()
            .map(|i| &self.bodies[i])
    }

    /// The frame registry: the root frame and one frame per body, built
    /// through gx-core's validated constructor at load time.
    pub fn registry(&self) -> Registry {
        self.registry.clone()
    }

    /// Orientation of a body frame's axes relative to its parent's axes at
    /// the epoch: `conj(parent to ICRF) * (body to ICRF)`. The root's axes
    /// are the ICRF axes. `None` for an unknown id; identity for the root.
    pub fn orientation(&self, frame_id: u64) -> Option<Quat> {
        if frame_id == self.root.frame_id {
            return Some(Quat::identity());
        }
        let body = self.body(frame_id)?;
        let parent = self.icrf_orientation(body.parent_frame_id);
        Some(normalized(parent.conjugate() * body.rotation.orientation()))
    }

    /// Body axes relative to ICRF axes; identity for the root.
    fn icrf_orientation(&self, frame_id: u64) -> Quat {
        self.body(frame_id)
            .map_or(Quat::identity(), |b| b.rotation.orientation())
    }

    /// Density at a point in a frame (meters from the frame origin, frame
    /// axes): the body's mean density inside the ball of mean radius, 0
    /// outside it, in the root frame, and in unknown frames.
    pub fn density_at(&self, frame_id: u64, p_in_frame: Vec3) -> Density {
        self.sample_at(frame_id, p_in_frame).density
    }

    /// The matter sample at a point in a frame: the body's [`Body::sample`]
    /// inside the ball of mean radius, [`Sample::VACUUM`] elsewhere.
    pub fn sample_at(&self, frame_id: u64, p_in_frame: Vec3) -> Sample {
        match self.body(frame_id) {
            Some(body) if body.contains(p_in_frame) => body.sample(),
            _ => Sample::VACUUM,
        }
    }

    /// Builds the registry frames in id order.
    fn frames(&self) -> Vec<Frame> {
        let root = Frame {
            frame_id: self.root.frame_id,
            parent_frame_id: ROOT_PARENT,
            root_extent: self.root.root_extent,
            max_depth: self.root.max_depth,
            mass: self.root.mass,
            position: Vec3::zero(),
            velocity: Vec3::zero(),
            orientation: Quat::identity(),
            angular_velocity: Vec3::zero(),
        };
        let bodies = self.bodies.iter().map(|b| Frame {
            frame_id: b.frame_id,
            parent_frame_id: b.parent_frame_id,
            root_extent: b.root_extent,
            max_depth: b.max_depth,
            mass: b.mass,
            position: b.position,
            velocity: b.velocity,
            orientation: self
                .orientation(b.frame_id)
                .expect("every body has an orientation"),
            angular_velocity: b.rotation.angular_velocity(),
        });
        std::iter::once(root).chain(bodies).collect()
    }
}

impl FromStr for System {
    type Err = ModelError;

    /// Parses and validates system data from TOML text.
    fn from_str(toml: &str) -> Result<System> {
        let raw: RawFile = toml::from_str(toml)?;
        build(raw)
    }
}

/// Scales a quaternion to unit norm; products of unit quaternions drift from
/// unit by a few ulps only.
fn normalized(q: Quat) -> Quat {
    q.normalized()
        .expect("a product of unit quaternions is not zero")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    epoch: RawEpoch,
    root: Option<RawRoot>,
    #[serde(default)]
    body: Vec<RawBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEpoch {
    tdb_seconds_since_j2000: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoot {
    frame_id: u64,
    name: String,
    mass_kg: f64,
    root_extent_m: f64,
    max_depth: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBody {
    frame_id: u64,
    parent_frame_id: u64,
    name: String,
    mass_kg: f64,
    mean_radius_m: f64,
    position_m: [f64; 3],
    velocity_m_per_s: [f64; 3],
    sidereal_orbit_period_s: Option<f64>,
    pole_ra_deg: f64,
    pole_dec_deg: f64,
    prime_meridian_deg: f64,
    rotation_rate_deg_per_day: f64,
    state: RawState,
    temperature_k: f64,
    albedo: [f64; 3],
    roughness: f64,
    attenuation_m2_per_kg: f64,
    root_extent_m: f64,
    max_depth: u8,
}

#[derive(Deserialize, Copy, Clone)]
#[serde(rename_all = "lowercase")]
enum RawState {
    Solid,
    Fluid,
    Gas,
    Plasma,
}

impl From<RawState> for State {
    fn from(s: RawState) -> State {
        match s {
            RawState::Solid => State::Solid,
            RawState::Fluid => State::Fluid,
            RawState::Gas => State::Gas,
            RawState::Plasma => State::Plasma,
        }
    }
}

/// Field checks for one frame.
struct Checker {
    frame_id: u64,
}

impl Checker {
    fn check(&self, field: &'static str, value: f64, ok: bool, rule: &'static str) -> Result<()> {
        if ok {
            Ok(())
        } else {
            Err(ModelError::Invalid {
                frame_id: self.frame_id,
                field,
                value,
                rule,
            })
        }
    }

    fn finite(&self, field: &'static str, v: f64) -> Result<()> {
        self.check(field, v, v.is_finite(), "is not finite")
    }

    fn positive(&self, field: &'static str, v: f64) -> Result<()> {
        self.check(
            field,
            v,
            v.is_finite() && v > 0.0,
            "is not finite and above 0",
        )
    }

    fn non_negative(&self, field: &'static str, v: f64) -> Result<()> {
        self.check(
            field,
            v,
            v.is_finite() && v >= 0.0,
            "is not finite and at least 0",
        )
    }

    fn unit(&self, field: &'static str, v: f64) -> Result<()> {
        self.check(field, v, (0.0..=1.0).contains(&v), "is not in 0 to 1")
    }

    fn vector(&self, field: &'static str, v: [f64; 3]) -> Result<()> {
        v.iter().try_for_each(|&c| self.finite(field, c))
    }

    fn depth(&self, v: u8) -> Result<()> {
        self.check(
            "max_depth",
            f64::from(v),
            v <= MAX_DEPTH,
            "is above the format's maximum of 31",
        )
    }
}

fn build(raw: RawFile) -> Result<System> {
    let epoch = raw.epoch.tdb_seconds_since_j2000;
    if !epoch.is_finite() {
        return Err(ModelError::Epoch(epoch));
    }
    let raw_root = raw.root.ok_or(ModelError::MissingRoot)?;
    let c = Checker {
        frame_id: raw_root.frame_id,
    };
    c.check(
        "frame_id",
        raw_root.frame_id as f64,
        raw_root.frame_id != ROOT_PARENT,
        "is the reserved root parent id",
    )?;
    c.non_negative("mass_kg", raw_root.mass_kg)?;
    c.positive("root_extent_m", raw_root.root_extent_m)?;
    c.depth(raw_root.max_depth)?;
    let root = RootFrame {
        frame_id: raw_root.frame_id,
        name: raw_root.name,
        mass: Kilograms::new(raw_root.mass_kg),
        root_extent: Meters::new(raw_root.root_extent_m),
        max_depth: raw_root.max_depth,
    };

    let mut bodies = raw
        .body
        .into_iter()
        .map(body_from_raw)
        .collect::<Result<Vec<_>>>()?;
    bodies.sort_by_key(|b| b.frame_id);

    let mut ids: Vec<u64> = bodies.iter().map(|b| b.frame_id).collect();
    ids.push(root.frame_id);
    ids.sort_unstable();
    if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(ModelError::DuplicateFrameId(w[0]));
    }
    for b in &bodies {
        if ids.binary_search(&b.parent_frame_id).is_err() || b.parent_frame_id == b.frame_id {
            return Err(ModelError::MissingParent {
                frame_id: b.frame_id,
                parent: b.parent_frame_id,
            });
        }
    }

    let mut system = System {
        epoch: Seconds::new(epoch),
        root,
        bodies,
        registry: Registry::empty(Seconds::new(epoch))?,
    };
    let registry = Registry::new(system.epoch, system.frames())?;
    // The single-registry union checks one root, parents, and no cycles.
    FrameTree::from_registries(std::slice::from_ref(&registry))?;
    system.registry = registry;
    Ok(system)
}

fn body_from_raw(b: RawBody) -> Result<Body> {
    let c = Checker {
        frame_id: b.frame_id,
    };
    c.check(
        "frame_id",
        b.frame_id as f64,
        b.frame_id != ROOT_PARENT,
        "is the reserved root parent id",
    )?;
    c.positive("mass_kg", b.mass_kg)?;
    c.positive("mean_radius_m", b.mean_radius_m)?;
    c.vector("position_m", b.position_m)?;
    c.vector("velocity_m_per_s", b.velocity_m_per_s)?;
    if let Some(p) = b.sidereal_orbit_period_s {
        c.positive("sidereal_orbit_period_s", p)?;
    }
    c.finite("pole_ra_deg", b.pole_ra_deg)?;
    c.check(
        "pole_dec_deg",
        b.pole_dec_deg,
        (-90.0..=90.0).contains(&b.pole_dec_deg),
        "is not in -90 to 90",
    )?;
    c.finite("prime_meridian_deg", b.prime_meridian_deg)?;
    c.finite("rotation_rate_deg_per_day", b.rotation_rate_deg_per_day)?;
    c.non_negative("temperature_k", b.temperature_k)?;
    b.albedo.iter().try_for_each(|&a| c.unit("albedo", a))?;
    c.unit("roughness", b.roughness)?;
    c.non_negative("attenuation_m2_per_kg", b.attenuation_m2_per_kg)?;
    c.positive("root_extent_m", b.root_extent_m)?;
    c.check(
        "root_extent_m",
        b.root_extent_m,
        b.root_extent_m >= 2.0 * b.mean_radius_m,
        "is smaller than the body's mean diameter",
    )?;
    c.depth(b.max_depth)?;
    let [px, py, pz] = b.position_m;
    let [vx, vy, vz] = b.velocity_m_per_s;
    Ok(Body {
        frame_id: b.frame_id,
        parent_frame_id: b.parent_frame_id,
        name: b.name,
        mass: Kilograms::new(b.mass_kg),
        mean_radius: Meters::new(b.mean_radius_m),
        position: Vec3::new(px, py, pz),
        velocity: Vec3::new(vx, vy, vz),
        sidereal_orbit_period: b.sidereal_orbit_period_s.map(Seconds::new),
        rotation: RotationElements {
            pole_ra_deg: b.pole_ra_deg,
            pole_dec_deg: b.pole_dec_deg,
            prime_meridian_deg: b.prime_meridian_deg,
            rate_deg_per_day: b.rotation_rate_deg_per_day,
        },
        matter: Matter {
            state: b.state.into(),
            temperature: Kelvin::new(b.temperature_k),
            albedo: b.albedo.map(Ratio::new),
            roughness: Ratio::new(b.roughness),
            attenuation: Attenuation::new(b.attenuation_m2_per_kg),
        },
        root_extent: Meters::new(b.root_extent_m),
        max_depth: b.max_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[epoch]
tdb_seconds_since_j2000 = 0.0

[root]
frame_id = 0
name = "origin"
mass_kg = 0.0
root_extent_m = 1.0e12
max_depth = 0

[[body]]
frame_id = 1
parent_frame_id = 0
name = "a"
mass_kg = 1.0e24
mean_radius_m = 1.0e6
position_m = [1.0e11, 0.0, 0.0]
velocity_m_per_s = [0.0, 3.0e4, 0.0]
pole_ra_deg = 0.0
pole_dec_deg = 90.0
prime_meridian_deg = 0.0
rotation_rate_deg_per_day = 360.0
state = "solid"
temperature_k = 250.0
albedo = [0.3, 0.3, 0.3]
roughness = 0.5
attenuation_m2_per_kg = 0.0
root_extent_m = 8.0e6
max_depth = 6
"#;

    fn with(from: &str, to: &str) -> String {
        assert!(MINIMAL.contains(from), "{from}");
        MINIMAL.replacen(from, to, 1)
    }

    #[test]
    fn minimal_loads() {
        let s: System = MINIMAL.parse().unwrap();
        assert_eq!(s.bodies().len(), 1);
        assert_eq!(s.registry().frames().len(), 2);
        assert!(s.body(0).is_none());
        assert!(s.body(1).is_some());
        assert_eq!(s.orientation(0), Some(Quat::identity()));
        assert_eq!(s.orientation(7), None);
    }

    #[test]
    fn density_inside_and_outside() {
        let s: System = MINIMAL.parse().unwrap();
        let b = s.body(1).unwrap();
        let expected = 1.0e24 / (4.0 / 3.0 * PI * 1.0e18);
        assert!((b.mean_density().value() / expected - 1.0).abs() < 1e-15);
        assert_eq!(s.density_at(1, Vec3::zero()), b.mean_density());
        assert_eq!(
            s.density_at(1, Vec3::new(1.0e6, 0.0, 0.0)),
            b.mean_density()
        );
        assert_eq!(
            s.density_at(1, Vec3::new(1.0e6 + 1.0, 0.0, 0.0)).value(),
            0.0
        );
        assert_eq!(s.density_at(0, Vec3::zero()).value(), 0.0);
        assert_eq!(s.density_at(9, Vec3::zero()).value(), 0.0);
        let inside = s.sample_at(1, Vec3::new(0.0, 5.0e5, 0.0));
        assert_eq!(inside.state, State::Solid);
        assert_eq!(inside.temperature, Kelvin::new(250.0));
        assert_eq!(inside.albedo, [Ratio::new(0.3); 3]);
        assert_eq!(s.sample_at(1, Vec3::new(0.0, 0.0, 2.0e6)), Sample::VACUUM);
    }

    #[test]
    fn rejects_bad_data() {
        let cases = [
            with("[root]\nframe_id = 0", "[root]\nframe_id = 1"),
            with("parent_frame_id = 0", "parent_frame_id = 5"),
            with("parent_frame_id = 0", "parent_frame_id = 1"),
            with("mass_kg = 1.0e24", "mass_kg = 0.0"),
            with("mass_kg = 0.0", "mass_kg = -1.0"),
            with("mean_radius_m = 1.0e6", "mean_radius_m = -1.0"),
            with("position_m = [1.0e11", "position_m = [nan"),
            with("velocity_m_per_s = [0.0", "velocity_m_per_s = [inf"),
            with("albedo = [0.3", "albedo = [1.3"),
            with("roughness = 0.5", "roughness = -0.5"),
            with("temperature_k = 250.0", "temperature_k = -1.0"),
            with(
                "attenuation_m2_per_kg = 0.0",
                "attenuation_m2_per_kg = -1.0",
            ),
            with("pole_dec_deg = 90.0", "pole_dec_deg = 91.0"),
            with("root_extent_m = 8.0e6", "root_extent_m = 1.0e6"),
            with("max_depth = 6", "max_depth = 32"),
            with("state = \"solid\"", "state = \"vacuum\""),
            with("roughness = 0.5", "roughness = 0.5\ncolor = 1"),
            with(
                "tdb_seconds_since_j2000 = 0.0",
                "tdb_seconds_since_j2000 = nan",
            ),
        ];
        for (i, case) in cases.iter().enumerate() {
            assert!(case.parse::<System>().is_err(), "case {i} loaded");
        }
    }

    #[test]
    fn missing_root_is_reported() {
        let text = MINIMAL
            .split("[[body]]")
            .next()
            .unwrap()
            .split("[root]")
            .next()
            .unwrap()
            .to_string();
        assert!(matches!(
            text.parse::<System>(),
            Err(ModelError::MissingRoot)
        ));
    }

    #[test]
    fn duplicate_ids_are_reported() {
        let body = MINIMAL.split("[[body]]").nth(1).unwrap();
        let text = format!("{MINIMAL}\n[[body]]{body}");
        assert!(matches!(
            text.parse::<System>(),
            Err(ModelError::DuplicateFrameId(1))
        ));
    }

    #[test]
    fn parent_cycles_are_rejected() {
        // Frames 1 and 2 name each other as parents; neither reaches the
        // root.
        let body = MINIMAL.split("[[body]]").nth(1).unwrap();
        let first = body.replacen("parent_frame_id = 0", "parent_frame_id = 2", 1);
        let second = body.replacen("frame_id = 1", "frame_id = 2", 1).replacen(
            "parent_frame_id = 0",
            "parent_frame_id = 1",
            1,
        );
        let head = MINIMAL.split("[[body]]").next().unwrap();
        let text = format!("{head}[[body]]{first}\n[[body]]{second}");
        assert!(matches!(
            text.parse::<System>(),
            Err(ModelError::Registry(_))
        ));
    }
}
