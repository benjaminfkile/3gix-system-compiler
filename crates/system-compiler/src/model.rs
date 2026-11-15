//! The planetary system: parsed and validated data, the frame registry at
//! any origin, and the per-body matter model.
//!
//! [`System`] is built from `data/system.toml` (see `docs/data-sources.md`
//! for where every number comes from). Loading checks the data, builds the
//! frame registry at the data epoch once through gx-core's validated
//! constructor (so the baked states, oblateness, and moments obey the rules
//! of `matter-format.md` section 5 and form a single frame tree), and
//! tabulates the polytropic profile of every gas and plasma body.
//!
//! Registry (`matter-format.md` section 5.2, `space-model.md` section 11
//! item 1): [`System::registry_at`] integrates the baked state vectors from
//! the data epoch to the origin with the library integrator
//! ([`crate::ephemeris`]) and emits one root frame for the system
//! barycenter, massless and at rest, and one frame per body with its mass,
//! position and velocity relative to its parent in ICRF axes at the origin,
//! orientation relative to its parent's axes and angular velocity about its
//! own axes from the IAU elements at the origin ([`crate::rotation`]), and
//! its `j2`, `reference_radius`, and `principal_moments`.
//!
//! Matter (`matter-format.md` section 3.3, documented in `docs/model.md`):
//! solid bodies are balls of uniform mean density; gas and plasma bodies are
//! polytropes matching their mass and mean radius ([`crate::polytrope`]).
//! Every body has one state, three-band albedo, roughness, and attenuation.
//! Outside the ball of mean radius is vacuum.

use std::path::Path;
use std::str::FromStr;

use core::f64::consts::PI;

use gx_core::error::ValidationError;
use gx_core::matter::{Sample, State};
use gx_core::registry::{Frame, FrameTree, Registry, MAX_DEPTH, ROOT_PARENT};
use gx_core::timescale::utc_ms_to_tdb_seconds;
use gx_core::units::{
    Attenuation, CubicMeters, Density, Kelvin, Kilograms, Meters, Quat, Ratio, Seconds, Vec3,
};
use serde::Deserialize;

use crate::ephemeris::{self, EphemerisError};
use crate::polytrope::{LaneEmden, Profile, ProfileError, Thermal, INDICES};
use crate::rotation::{PeriodicTerm, RotationElements};

/// Pressure of the reference level of a gas body's temperature, pascals:
/// 1 bar (`docs/model.md`, "Temperature").
pub const REFERENCE_PRESSURE_PA: f64 = 1.0e5;

/// Why system data failed to load, or a registry could not be built.
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
    /// A body's polytropic profile could not be built.
    #[error("frame_id {frame_id}: {source}")]
    Profile {
        /// The body's frame id.
        frame_id: u64,
        /// The underlying error.
        source: ProfileError,
    },
    /// The epoch is not finite.
    #[error("epoch tdb_seconds_since_j2000 = {0} is not finite")]
    Epoch(f64),
    /// The origin is too far from the data epoch to integrate to.
    #[error("{0}")]
    Ephemeris(#[from] EphemerisError),
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

/// What a body is made of (`docs/model.md`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Matter {
    /// State of the matter; never vacuum.
    pub state: State,
    /// Temperature: the uniform temperature of a solid body, the 1 bar
    /// temperature of a gas body, and the effective (photosphere)
    /// temperature of a plasma body.
    pub temperature: Kelvin,
    /// Temperature at the center of a polytrope scaled to a central
    /// temperature (the plasma body); `None` otherwise.
    pub central_temperature: Option<Kelvin>,
    /// Reflectance in the three bands of `gx_core::radiance::BAND_EDGES`,
    /// long to short wavelength.
    pub albedo: [Ratio; 3],
    /// Microfacet roughness.
    pub roughness: Ratio,
    /// Mass attenuation coefficient; 0 for a hard surface. For a polytrope,
    /// the value set by the photosphere condition.
    pub attenuation: Attenuation,
}

/// How density varies inside a body.
#[derive(Clone, Debug, PartialEq)]
pub enum Structure {
    /// The mean density everywhere inside the ball of mean radius.
    Uniform,
    /// A hydrostatic polytrope matching mass and mean radius.
    Polytrope(Profile),
}

/// One body: its frame, its state at the data epoch, its rotation, its
/// gravity field and inertia, and its matter.
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
    /// Position relative to the parent at the data epoch, meters, ICRF axes.
    pub position: Vec3,
    /// Velocity relative to the parent at the data epoch, meters per second,
    /// ICRF axes.
    pub velocity: Vec3,
    /// Published sidereal orbital period about the parent, a check value
    /// that is never compiled.
    pub sidereal_orbit_period: Option<Seconds>,
    /// IAU rotational elements.
    pub rotation: RotationElements,
    /// Unnormalized second zonal harmonic, dimensionless.
    pub j2: f64,
    /// The radius `j2` is quoted against.
    pub reference_radius: Meters,
    /// Principal moments of inertia about the body axes, kg m^2.
    pub principal_moments: Vec3,
    /// The matter model.
    pub matter: Matter,
    /// Uniform or polytropic interior.
    pub structure: Structure,
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

    /// Density at distance `r` from the center: the mean density or the
    /// polytrope's, 0 beyond the mean radius.
    pub fn density_at_radius(&self, r: f64) -> Density {
        let radius = self.mean_radius.value();
        if r > radius {
            return Density::new(0.0);
        }
        match &self.structure {
            Structure::Uniform => self.mean_density(),
            Structure::Polytrope(p) => Density::new(p.density_at(r)),
        }
    }

    /// Temperature at distance `r` from the center, inside the body.
    pub fn temperature_at_radius(&self, r: f64) -> Kelvin {
        match &self.structure {
            Structure::Uniform => self.matter.temperature,
            Structure::Polytrope(p) => Kelvin::new(p.temperature_at(r)),
        }
    }

    /// The matter sample at a point in the body frame: the body's matter at
    /// the local density and temperature inside the ball of mean radius
    /// (vacuum where a polytrope's density reaches 0), [`Sample::VACUUM`]
    /// outside it.
    pub fn sample_at(&self, p_in_frame: Vec3) -> Sample {
        if !self.contains(p_in_frame) {
            return Sample::VACUUM;
        }
        let r = p_in_frame.length();
        let density = self.density_at_radius(r);
        if density.value() <= 0.0 {
            return Sample::VACUUM;
        }
        Sample {
            density,
            temperature: self.temperature_at_radius(r),
            ..self.uniform_sample()
        }
    }

    /// The body's matter at its mean density and [`Matter::temperature`]:
    /// every channel but density and temperature is the same everywhere
    /// inside.
    pub fn uniform_sample(&self) -> Sample {
        Sample {
            density: self.mean_density(),
            state: self.matter.state,
            temperature: self.matter.temperature,
            albedo: self.matter.albedo,
            roughness: self.matter.roughness,
            attenuation: self.matter.attenuation,
            radiating_temperature: self.matter.temperature,
        }
    }

    /// The frame record at the data epoch, orientation relative to the ICRF
    /// axes (the caller composes it with the parent's).
    fn frame(&self, epoch: Seconds, orientation: Quat) -> Frame {
        Frame {
            frame_id: self.frame_id,
            parent_frame_id: self.parent_frame_id,
            root_extent: self.root_extent,
            max_depth: self.max_depth,
            mass: self.mass,
            position: self.position,
            velocity: self.velocity,
            orientation,
            angular_velocity: self.rotation.at(epoch).angular_velocity(),
            j2: self.j2,
            reference_radius: self.reference_radius,
            principal_moments: self.principal_moments,
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
    /// The registry at the data epoch: the baked state vectors.
    epoch_registry: Registry,
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

    /// Epoch of every baked state vector (the data epoch), seconds of TDB
    /// since J2000.
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

    /// The frame registry at the data epoch: the baked state vectors, with
    /// orientation and spin from the IAU elements at the data epoch.
    pub fn epoch_registry(&self) -> &Registry {
        &self.epoch_registry
    }

    /// The frame registry at the origin `origin_utc_ms` (UTC milliseconds,
    /// Unix-style): the baked states integrated to the origin in TDB with
    /// [`ephemeris::integrate`], orientation and spin from the IAU elements
    /// at the origin. Its epoch is the origin in TDB seconds since J2000.
    ///
    /// Fails if the origin is more than [`ephemeris::MAX_INTERVAL_S`] from
    /// the data epoch.
    pub fn registry_at(&self, origin_utc_ms: i64) -> Result<Registry> {
        let tdb = Seconds::new(utc_ms_to_tdb_seconds(origin_utc_ms));
        self.registry_at_tdb(tdb)
    }

    /// [`System::registry_at`] for an origin given in TDB seconds since
    /// J2000.
    pub fn registry_at_tdb(&self, tdb: Seconds) -> Result<Registry> {
        let integrated = ephemeris::integrate(&self.epoch_registry, tdb)?;
        let mut frames = vec![self.root_frame()];
        for b in &self.bodies {
            let state = integrated
                .state(b.frame_id)
                .expect("every body is in the integrated tree");
            let mut f = b.frame(tdb, self.orientation_at(b.frame_id, tdb));
            f.position = state.position;
            f.velocity = state.velocity;
            frames.push(f);
        }
        Ok(Registry::new(tdb, frames)?)
    }

    /// Orientation of a body frame's axes relative to its parent's axes at
    /// `tdb`: `conj(parent to ICRF) * (body to ICRF)`, both from the IAU
    /// elements at `tdb`. The root's axes are the ICRF axes. Identity for
    /// the root and for an unknown id.
    pub fn orientation_at(&self, frame_id: u64, tdb: Seconds) -> Quat {
        let Some(body) = self.body(frame_id) else {
            return Quat::identity();
        };
        let parent = self.icrf_orientation(body.parent_frame_id, tdb);
        normalized(parent.conjugate() * body.rotation.at(tdb).orientation())
    }

    /// Body axes relative to ICRF axes at `tdb`; identity for the root.
    fn icrf_orientation(&self, frame_id: u64, tdb: Seconds) -> Quat {
        self.body(frame_id)
            .map_or(Quat::identity(), |b| b.rotation.at(tdb).orientation())
    }

    /// Density at a point in a frame (meters from the frame origin, frame
    /// axes): the body's local density inside the ball of mean radius, 0
    /// outside it, in the root frame, and in unknown frames.
    pub fn density_at(&self, frame_id: u64, p_in_frame: Vec3) -> Density {
        self.sample_at(frame_id, p_in_frame).density
    }

    /// The matter sample at a point in a frame: [`Body::sample_at`] for a
    /// body frame, [`Sample::VACUUM`] elsewhere.
    pub fn sample_at(&self, frame_id: u64, p_in_frame: Vec3) -> Sample {
        match self.body(frame_id) {
            Some(body) => body.sample_at(p_in_frame),
            None => Sample::VACUUM,
        }
    }

    fn root_frame(&self) -> Frame {
        Frame {
            frame_id: self.root.frame_id,
            parent_frame_id: ROOT_PARENT,
            root_extent: self.root.root_extent,
            max_depth: self.root.max_depth,
            mass: self.root.mass,
            position: Vec3::zero(),
            velocity: Vec3::zero(),
            orientation: Quat::identity(),
            angular_velocity: Vec3::zero(),
            j2: 0.0,
            reference_radius: Meters::new(0.0),
            principal_moments: Vec3::zero(),
        }
    }

    /// The registry frames at the data epoch, in id order.
    fn epoch_frames(&self) -> Vec<Frame> {
        let bodies = self
            .bodies
            .iter()
            .map(|b| b.frame(self.epoch, self.orientation_at(b.frame_id, self.epoch)));
        std::iter::once(self.root_frame()).chain(bodies).collect()
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
struct RawTerm {
    angle_deg: [f64; 3],
    ra_deg: f64,
    dec_deg: f64,
    pm_deg: f64,
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
    pole_ra_deg: [f64; 2],
    pole_dec_deg: [f64; 2],
    prime_meridian_deg: [f64; 3],
    #[serde(default)]
    rotation_terms: Vec<RawTerm>,
    j2: f64,
    reference_radius_m: f64,
    principal_moments_kg_m2: [f64; 3],
    state: RawState,
    polytropic_index: Option<u8>,
    temperature_k: f64,
    central_temperature_k: Option<f64>,
    albedo: [f64; 3],
    roughness: f64,
    attenuation_m2_per_kg: Option<f64>,
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

    fn all_finite(&self, field: &'static str, v: &[f64]) -> Result<()> {
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

    // One solution per index, shared by every body of that index.
    let solutions: Vec<LaneEmden> = INDICES.iter().map(|&n| LaneEmden::solve(n)).collect();
    let mut bodies = raw
        .body
        .into_iter()
        .map(|b| body_from_raw(b, &solutions))
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
        epoch_registry: Registry::empty(),
    };
    let registry = Registry::new(system.epoch, system.epoch_frames())?;
    // The single-registry union checks one root, parents, and no cycles.
    FrameTree::from_registries(std::slice::from_ref(&registry))?;
    system.epoch_registry = registry;
    Ok(system)
}

fn body_from_raw(b: RawBody, solutions: &[LaneEmden]) -> Result<Body> {
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
    c.all_finite("position_m", &b.position_m)?;
    c.all_finite("velocity_m_per_s", &b.velocity_m_per_s)?;
    if let Some(p) = b.sidereal_orbit_period_s {
        c.positive("sidereal_orbit_period_s", p)?;
    }
    c.all_finite("pole_ra_deg", &b.pole_ra_deg)?;
    c.all_finite("pole_dec_deg", &b.pole_dec_deg)?;
    c.check(
        "pole_dec_deg",
        b.pole_dec_deg[0],
        (-90.0..=90.0).contains(&b.pole_dec_deg[0]),
        "is not in -90 to 90",
    )?;
    c.all_finite("prime_meridian_deg", &b.prime_meridian_deg)?;
    for t in &b.rotation_terms {
        c.all_finite("rotation_terms", &t.angle_deg)?;
        c.all_finite("rotation_terms", &[t.ra_deg, t.dec_deg, t.pm_deg])?;
    }
    c.non_negative("j2", b.j2)?;
    c.non_negative("reference_radius_m", b.reference_radius_m)?;
    c.all_finite("principal_moments_kg_m2", &b.principal_moments_kg_m2)?;
    c.non_negative("temperature_k", b.temperature_k)?;
    b.albedo.iter().try_for_each(|&a| c.unit("albedo", a))?;
    c.unit("roughness", b.roughness)?;
    // A polytrope takes its attenuation from the photosphere condition
    // (docs/model.md, "Temperature"); every other body states it.
    match b.attenuation_m2_per_kg {
        Some(a) => {
            c.non_negative("attenuation_m2_per_kg", a)?;
            c.check(
                "attenuation_m2_per_kg",
                a,
                b.polytropic_index.is_none(),
                "is derived for a polytrope and must not be given",
            )?;
        }
        None => c.check(
            "attenuation_m2_per_kg",
            f64::NAN,
            b.polytropic_index.is_some(),
            "must be given unless the body is a polytrope",
        )?,
    }
    c.positive("root_extent_m", b.root_extent_m)?;
    c.check(
        "root_extent_m",
        b.root_extent_m,
        b.root_extent_m >= 2.0 * b.mean_radius_m,
        "is smaller than the body's mean diameter",
    )?;
    c.depth(b.max_depth)?;

    let state: State = b.state.into();
    let fluid = matches!(state, State::Gas | State::Plasma);
    let index = b.polytropic_index.map_or(0.0, f64::from);
    c.check(
        "polytropic_index",
        index,
        b.polytropic_index.is_some() == fluid,
        "must be given for gas and plasma bodies and only for them",
    )?;
    if let Some(t) = b.central_temperature_k {
        c.positive("central_temperature_k", t)?;
        c.check(
            "central_temperature_k",
            t,
            fluid,
            "is only meaningful for a polytrope",
        )?;
    }
    let structure = match b.polytropic_index {
        None => Structure::Uniform,
        Some(n) => {
            let i = INDICES
                .iter()
                .position(|&k| k == n)
                .ok_or(ModelError::Invalid {
                    frame_id: b.frame_id,
                    field: "polytropic_index",
                    value: f64::from(n),
                    rule: "is not 1 or 3",
                })?;
            let thermal = match b.central_temperature_k {
                Some(central) => Thermal::Central {
                    central,
                    effective: b.temperature_k,
                },
                None => Thermal::Reference {
                    pressure: REFERENCE_PRESSURE_PA,
                    temperature: b.temperature_k,
                },
            };
            let profile = Profile::new(&solutions[i], b.mass_kg, b.mean_radius_m, thermal)
                .map_err(|source| ModelError::Profile {
                    frame_id: b.frame_id,
                    source,
                })?;

            Structure::Polytrope(profile)
        }
    };

    let attenuation = match &structure {
        Structure::Polytrope(p) => Some(p.attenuation()),
        Structure::Uniform => None,
    }
    .or(b.attenuation_m2_per_kg)
    .unwrap_or(0.0);
    let [px, py, pz] = b.position_m;
    let [vx, vy, vz] = b.velocity_m_per_s;
    let [ma, mb, mc] = b.principal_moments_kg_m2;
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
            terms: b
                .rotation_terms
                .iter()
                .map(|t| PeriodicTerm {
                    angle_deg: t.angle_deg,
                    ra_deg: t.ra_deg,
                    dec_deg: t.dec_deg,
                    pm_deg: t.pm_deg,
                })
                .collect(),
        },
        j2: b.j2,
        reference_radius: Meters::new(b.reference_radius_m),
        principal_moments: Vec3::new(ma, mb, mc),
        matter: Matter {
            state,
            temperature: Kelvin::new(b.temperature_k),
            central_temperature: b.central_temperature_k.map(Kelvin::new),
            albedo: b.albedo.map(Ratio::new),
            roughness: Ratio::new(b.roughness),
            attenuation: Attenuation::new(attenuation),
        },
        structure,
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
pole_ra_deg = [0.0, 0.0]
pole_dec_deg = [90.0, 0.0]
prime_meridian_deg = [0.0, 360.0, 0.0]
j2 = 1.0e-3
reference_radius_m = 1.0e6
principal_moments_kg_m2 = [3.9e35, 3.9e35, 4.0e35]
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

    fn gas() -> String {
        with("state = \"solid\"", "state = \"gas\"\npolytropic_index = 1")
            .replacen("mass_kg = 1.0e24", "mass_kg = 1.0e27", 1)
            .replacen("mean_radius_m = 1.0e6", "mean_radius_m = 7.0e7", 1)
            .replacen("root_extent_m = 8.0e6", "root_extent_m = 5.6e8", 1)
            .replacen("attenuation_m2_per_kg = 0.0\n", "", 1)
    }

    #[test]
    fn minimal_loads() {
        let s: System = MINIMAL.parse().unwrap();
        assert_eq!(s.bodies().len(), 1);
        assert_eq!(s.epoch_registry().frames().len(), 2);
        assert!(s.body(0).is_none());
        assert!(s.body(1).is_some());
        assert_eq!(s.orientation_at(0, Seconds::new(0.0)), Quat::identity());
        let f = s.epoch_registry().frames()[1];
        assert_eq!(f.j2, 1.0e-3);
        assert_eq!(f.principal_moments, Vec3::new(3.9e35, 3.9e35, 4.0e35));
        assert_eq!(s.body(1).unwrap().structure, Structure::Uniform);
    }

    #[test]
    fn registry_at_the_data_epoch_is_the_baked_one() {
        let s: System = MINIMAL.parse().unwrap();
        assert_eq!(
            &s.registry_at_tdb(Seconds::new(0.0)).unwrap(),
            s.epoch_registry()
        );
        let later = s.registry_at_tdb(Seconds::new(86_400.0)).unwrap();
        assert_eq!(later.epoch().value(), 86_400.0);
        assert_ne!(
            later.frames()[1].position,
            s.epoch_registry().frames()[1].position
        );
        assert!(s.registry_at_tdb(Seconds::new(1.0e12)).is_err());
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
    fn gas_bodies_are_polytropes() {
        let s: System = gas().parse().unwrap();
        let b = s.body(1).unwrap();
        assert!(matches!(b.structure, Structure::Polytrope(_)));
        let center = s.density_at(1, Vec3::zero()).value();
        assert!((center / b.mean_density().value() - PI * PI / 3.0).abs() < 1e-6);
        let edge = s.sample_at(1, Vec3::new(7.0e7, 0.0, 0.0));
        assert_eq!(edge, Sample::VACUUM);
        let mid = s.sample_at(1, Vec3::new(3.5e7, 0.0, 0.0));
        assert_eq!(mid.state, State::Gas);
        assert!(mid.temperature.value() >= 250.0);
        // The attenuation is derived, and a gas body may not state one.
        assert!(b.matter.attenuation.value() > 0.0);
        let stated = gas().replacen(
            "roughness = 0.5",
            "roughness = 0.5\nattenuation_m2_per_kg = 1.0",
            1,
        );
        assert!(stated.parse::<System>().is_err());
        // A solid body must state one.
        let missing = MINIMAL.replacen("attenuation_m2_per_kg = 0.0\n", "", 1);
        assert!(missing.parse::<System>().is_err());
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
            with("pole_dec_deg = [90.0", "pole_dec_deg = [91.0"),
            with("root_extent_m = 8.0e6", "root_extent_m = 1.0e6"),
            with("max_depth = 6", "max_depth = 32"),
            with("state = \"solid\"", "state = \"vacuum\""),
            with("roughness = 0.5", "roughness = 0.5\ncolor = 1"),
            with(
                "tdb_seconds_since_j2000 = 0.0",
                "tdb_seconds_since_j2000 = nan",
            ),
            with("j2 = 1.0e-3", "j2 = -1.0e-3"),
            // j2 above 0 needs a reference radius (gx-core rule).
            with("reference_radius_m = 1.0e6", "reference_radius_m = 0.0"),
            // Moments break the triangle rule.
            with("[3.9e35, 3.9e35, 4.0e35]", "[1.0e35, 1.0e35, 4.0e35]"),
            // A solid body has no polytropic index; a gas body needs one.
            with(
                "state = \"solid\"",
                "state = \"solid\"\npolytropic_index = 1",
            ),
            with("state = \"solid\"", "state = \"gas\""),
            gas().replacen("polytropic_index = 1", "polytropic_index = 2", 1),
            // A gas polytrope with a reference temperature needs index 1.
            gas().replacen("polytropic_index = 1", "polytropic_index = 3", 1),
            with(
                "temperature_k = 250.0",
                "temperature_k = 250.0\ncentral_temperature_k = 1.0e7",
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
