//! Weapon profiles: one data model for every gun.
//!
//! A profile describes a real-world weapon in the units its makers use (grains, ft/s, inches, pounds) as a
//! combination of polymorphic parts: the [`Projectile`] (a bullet by caliber, or a slug or shot by gauge), the
//! [`Action`], the [`Feed`], the [`FireMode`]s and the recoil [`Mitigation`]. [`resolve`] turns any profile into
//! physical [`WeaponStats`] (muzzle energy, momentum, free and felt recoil) with one set of formulas, then into
//! the values the game's weapon components use. Adding a gun is adding data; no gun is special-cased in code.
//!
//! Scenes keep profiles in the `weapon_profiles` resource and game-feel constants in `weapon_tuning`; an entity
//! selects one with a `WeaponProfile` component (`{"profile": "<id>"}`), and the scene instantiator merges the
//! resolved component values over that entity's authored components.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Kilograms per grain.
pub const GRAIN_KG: f64 = 64.798_91e-6;
/// Grains per pound.
pub const GRAINS_PER_POUND: f64 = 7000.0;
/// Metres per foot.
pub const FOOT_M: f64 = 0.3048;
/// Kilograms per pound.
pub const POUND_KG: f64 = 0.453_592_37;
/// Joules per foot-pound of energy.
pub const FOOT_POUND_J: f64 = 1.355_817_948_331_4;
/// Pure lead, pounds per cubic inch: a bore of gauge `n` fits a lead ball of 1/n pound.
const LEAD_POUNDS_PER_CUBIC_INCH: f64 = 0.409_67;

/// A real-world weapon, described in its own units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponProfile {
    pub name: String,
    pub class: WeaponClass,
    pub cartridge: Cartridge,
    pub barrel_in: f64,
    /// Loaded weapon mass, the mass that recoils.
    pub mass_lb: f64,
    pub action: Action,
    pub feed: Feed,
    /// In the order the player cycles through them; the first is the starting mode.
    pub fire_modes: Vec<FireMode>,
    /// Cyclic rate for automatic actions, practical rate otherwise.
    pub fire_rate_rpm: f64,
    #[serde(default)]
    pub mitigation: Mitigation,
    #[serde(default)]
    pub handling: Handling,
    /// The `first_person_manifest` asset holding this weapon's first-person footage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footage: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponClass {
    Revolver,
    Pistol,
    SubmachineGun,
    Carbine,
    Rifle,
    Shotgun,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cartridge {
    pub name: String,
    pub projectile: Projectile,
    /// Propellant charge; its gases leave the muzzle too and add to recoil.
    pub powder_grains: f64,
    pub muzzle_velocity_fps: f64,
    /// Powder-gas exit velocity as a multiple of the muzzle velocity (`weapon_tuning` default otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gas_velocity_factor: Option<f64>,
}

/// What leaves the barrel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Projectile {
    /// One bullet; caliber is the nominal bore in inches (.500 = 0.5).
    Bullet { caliber_in: f64, grains: f64 },
    /// One slug from a shotgun bore of the given gauge.
    Slug { gauge: f64, grains: f64 },
    /// A charge of `pellets` pellets, spread in a cone of `spread_deg` (full angle).
    Shot { gauge: f64, pellets: u32, pellet_grains: f64, spread_deg: f64 },
}

/// How a shot is fired and the next one readied. Self-loading operations spread the recoil impulse over time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Revolver { double_action: bool },
    SemiAutomatic { operation: Operation },
    SelectFire { operation: Operation },
    Pump,
    Bolt,
    Lever,
    BreakAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Gas,
    ShortRecoil,
    Blowback,
    DelayedBlowback,
}

/// How rounds get to the chamber and how the weapon is reloaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Feed {
    Cylinder { rounds: u32, loader: Loader },
    DetachableMagazine { capacity: u32, spares: u32, reload_seconds: f64 },
    TubeMagazine { capacity: u32, reserve: u32, round_seconds: f64 },
    InternalMagazine { capacity: u32, reserve: u32, round_seconds: f64 },
}

/// How a revolver cylinder is refilled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Loader {
    /// All chambers at once from a loaded speedloader.
    Speedloader { spares: u32, reload_seconds: f64 },
    /// One round at a time from loose rounds.
    Loose { reserve: u32, round_seconds: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FireMode {
    Single,
    Burst { rounds: u32 },
    Auto,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mitigation {
    pub muzzle_device: MuzzleDevice,
    /// Fraction of felt recoil a recoil pad or padded grip absorbs (0..0.5).
    pub recoil_pad: f64,
}

/// A muzzle device turns part of the powder gases' momentum away from the shooter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MuzzleDevice {
    #[default]
    None,
    /// Ports that vent gas mostly upward: typically 0.15-0.35 of the gas momentum.
    Compensator { gas_redirect: f64 },
    /// Side baffles: typically 0.4-0.7 of the gas momentum.
    Brake { gas_redirect: f64 },
    /// Traps and slows gas, and adds its own mass.
    Suppressor { gas_redirect: f64, mass_lb: f64 },
}

/// Optional per-weapon game-feel overrides of `weapon_tuning`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Handling {
    pub horizontal_ratio: Option<f64>,
    pub rise_seconds: Option<f64>,
    pub damage_scale: Option<f64>,
}

/// Game-feel constants that turn physics into gameplay values, shared by every weapon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeaponTuning {
    /// Damage per kilojoule of muzzle energy, per projectile.
    pub damage_per_kj: f64,
    /// View kick in degrees per square root of felt recoil energy (ft·lbf).
    pub recoil_degrees_per_sqrt_ftlbf: f64,
    pub horizontal_ratio: f64,
    pub rise_seconds: f64,
    pub gas_velocity_factor: f64,
}
impl Default for WeaponTuning {
    fn default() -> Self {
        Self {
            damage_per_kj: 22.3,
            recoil_degrees_per_sqrt_ftlbf: 0.31,
            horizontal_ratio: 0.22,
            rise_seconds: 0.045,
            gas_velocity_factor: 1.75,
        }
    }
}

/// The physics of one shot from a profile, in both unit systems where useful.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeaponStats {
    pub name: String,
    pub class: WeaponClass,
    pub cartridge: String,
    pub bore_in: f64,
    pub projectiles: u32,
    pub projectile_grains: f64,
    pub payload_grains: f64,
    pub projectile_mass_kg: f64,
    pub muzzle_velocity_fps: f64,
    pub muzzle_velocity_mps: f64,
    pub muzzle_energy_ftlbf: f64,
    pub muzzle_energy_j: f64,
    pub projectile_energy_j: f64,
    pub momentum_ns: f64,
    pub recoil_impulse_ns: f64,
    pub recoil_velocity_fps: f64,
    pub free_recoil_ftlbf: f64,
    pub felt_recoil_ftlbf: f64,
    /// Felt recoil removed by the muzzle device, pad and operation, as a fraction of the unmitigated free recoil.
    pub recoil_reduction: f64,
    pub fire_rate_rpm: f64,
    pub fire_interval_s: f64,
    pub capacity: u32,
    pub damage_per_projectile: f64,
    pub recoil_vertical_degrees: f64,
    pub recoil_horizontal_degrees: f64,
}

/// A resolved profile: its physics and the component values it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWeapon {
    pub stats: WeaponStats,
    /// Component name to the fields to merge over the entity's authored component.
    pub components: BTreeMap<String, Value>,
}

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("weapon profile: {what}"))
    }
}
fn within(value: f64, low: f64, high: f64) -> bool {
    value.is_finite() && value >= low && value <= high
}

/// Bore diameter of a gauge: the diameter of a pure lead ball weighing 1/gauge pound.
pub fn gauge_bore_in(gauge: f64) -> f64 {
    (6.0 / (std::f64::consts::PI * LEAD_POUNDS_PER_CUBIC_INCH * gauge)).cbrt()
}

impl WeaponProfile {
    pub fn validate(&self) -> Result<(), String> {
        check(!self.name.trim().is_empty() && self.name.len() <= 80, "name must be 1-80 characters")?;
        check(within(self.barrel_in, 1.0, 40.0), "barrel_in must be 1-40")?;
        check(within(self.mass_lb, 0.5, 40.0), "mass_lb must be 0.5-40")?;
        check(within(self.fire_rate_rpm, 1.0, 2000.0), "fire_rate_rpm must be 1-2000")?;
        let c = &self.cartridge;
        check(within(c.powder_grains, 0.0, 300.0), "powder_grains must be 0-300")?;
        check(within(c.muzzle_velocity_fps, 100.0, 5000.0), "muzzle_velocity_fps must be 100-5000")?;
        if let Some(f) = c.gas_velocity_factor {
            check(within(f, 0.5, 3.0), "gas_velocity_factor must be 0.5-3")?;
        }
        match c.projectile {
            Projectile::Bullet { caliber_in, grains } => {
                check(within(caliber_in, 0.1, 1.0), "caliber_in must be 0.1-1")?;
                check(within(grains, 10.0, 1000.0), "bullet grains must be 10-1000")?;
            }
            Projectile::Slug { gauge, grains } => {
                check(within(gauge, 2.0, 60.0), "gauge must be 2-60")?;
                check(within(grains, 50.0, 1500.0), "slug grains must be 50-1500")?;
            }
            Projectile::Shot { gauge, pellets, pellet_grains, spread_deg } => {
                check(within(gauge, 2.0, 60.0), "gauge must be 2-60")?;
                check((1..=600).contains(&pellets), "pellets must be 1-600")?;
                check(within(pellet_grains, 0.5, 200.0), "pellet_grains must be 0.5-200")?;
                check(within(spread_deg, 0.0, 20.0), "spread_deg must be 0-20")?;
            }
        }
        check(!self.fire_modes.is_empty() && self.fire_modes.len() <= 3, "fire_modes needs 1-3 modes")?;
        for (i, mode) in self.fire_modes.iter().enumerate() {
            if let FireMode::Burst { rounds } = mode {
                check((2..=10).contains(rounds), "burst rounds must be 2-10")?;
            }
            check(!self.fire_modes[..i].contains(mode), "fire_modes must not repeat")?;
        }
        let automatic = self.fire_modes.iter().any(|m| !matches!(m, FireMode::Single));
        check(
            !automatic || matches!(self.action, Action::SelectFire { .. }),
            "only a select-fire action has burst or automatic modes",
        )?;
        let (capacity, reload) = match &self.feed {
            Feed::Cylinder { rounds, loader } => {
                check(matches!(self.action, Action::Revolver { .. }), "a cylinder feed needs a revolver action")?;
                match loader {
                    Loader::Speedloader { spares, reload_seconds } => {
                        check(*spares <= 10, "speedloader spares must be 0-10")?;
                        (*rounds, *reload_seconds)
                    }
                    Loader::Loose { reserve, round_seconds } => {
                        check(*reserve <= 500, "loose reserve must be 0-500")?;
                        (*rounds, *round_seconds)
                    }
                }
            }
            Feed::DetachableMagazine { capacity, spares, reload_seconds } => {
                check(*spares <= 10, "magazine spares must be 0-10")?;
                (*capacity, *reload_seconds)
            }
            Feed::TubeMagazine { capacity, reserve, round_seconds }
            | Feed::InternalMagazine { capacity, reserve, round_seconds } => {
                check(*reserve <= 500, "reserve must be 0-500")?;
                (*capacity, *round_seconds)
            }
        };
        check((1..=200).contains(&capacity), "capacity must be 1-200")?;
        check(within(reload, 0.05, 15.0), "reload times must be 0.05-15 s")?;
        let m = &self.mitigation;
        check(within(m.recoil_pad, 0.0, 0.5), "recoil_pad must be 0-0.5")?;
        match m.muzzle_device {
            MuzzleDevice::None => {}
            MuzzleDevice::Compensator { gas_redirect } | MuzzleDevice::Brake { gas_redirect } => {
                check(within(gas_redirect, 0.0, 0.9), "gas_redirect must be 0-0.9")?;
            }
            MuzzleDevice::Suppressor { gas_redirect, mass_lb } => {
                check(within(gas_redirect, 0.0, 0.9), "gas_redirect must be 0-0.9")?;
                check(within(mass_lb, 0.0, 5.0), "suppressor mass_lb must be 0-5")?;
            }
        }
        let h = &self.handling;
        for (value, low, high, what) in [
            (h.horizontal_ratio, 0.0, 2.0, "handling.horizontal_ratio must be 0-2"),
            (h.rise_seconds, 0.005, 0.5, "handling.rise_seconds must be 0.005-0.5"),
            (h.damage_scale, 0.0, 10.0, "handling.damage_scale must be 0-10"),
        ] {
            if let Some(v) = value {
                check(within(v, low, high), what)?;
            }
        }
        if let Some(footage) = &self.footage {
            check(!footage.is_empty() && footage.len() <= 128, "footage must name an asset")?;
        }
        Ok(())
    }

    /// Rounds the weapon holds when loaded.
    pub fn capacity(&self) -> u32 {
        match &self.feed {
            Feed::Cylinder { rounds, .. } => *rounds,
            Feed::DetachableMagazine { capacity, .. }
            | Feed::TubeMagazine { capacity, .. }
            | Feed::InternalMagazine { capacity, .. } => *capacity,
        }
    }

    /// The fraction of the peak recoil impulse a self-loading operation leaves for the shooter to feel.
    fn operation_feel(&self) -> f64 {
        match &self.action {
            Action::SemiAutomatic { operation } | Action::SelectFire { operation } => match operation {
                Operation::Gas => 0.8,
                Operation::ShortRecoil | Operation::DelayedBlowback => 0.9,
                Operation::Blowback => 0.95,
            },
            _ => 1.0,
        }
    }
}

impl WeaponTuning {
    pub fn validate(&self) -> Result<(), String> {
        check(within(self.damage_per_kj, 0.0, 1000.0), "tuning damage_per_kj must be 0-1000")?;
        check(within(self.recoil_degrees_per_sqrt_ftlbf, 0.0, 10.0), "tuning recoil_degrees_per_sqrt_ftlbf must be 0-10")?;
        check(within(self.horizontal_ratio, 0.0, 2.0), "tuning horizontal_ratio must be 0-2")?;
        check(within(self.rise_seconds, 0.005, 0.5), "tuning rise_seconds must be 0.005-0.5")?;
        check(within(self.gas_velocity_factor, 0.5, 3.0), "tuning gas_velocity_factor must be 0.5-3")
    }
}

/// Resolves a profile into its physics and the game component values it implies.
pub fn resolve(profile: &WeaponProfile, tuning: &WeaponTuning) -> Result<ResolvedWeapon, String> {
    profile.validate()?;
    tuning.validate()?;
    let c = &profile.cartridge;
    let (bore_in, projectiles, projectile_grains, spread_deg) = match c.projectile {
        Projectile::Bullet { caliber_in, grains } => (caliber_in, 1, grains, 0.0),
        Projectile::Slug { gauge, grains } => (gauge_bore_in(gauge), 1, grains, 0.0),
        Projectile::Shot { gauge, pellets, pellet_grains, spread_deg } => {
            (gauge_bore_in(gauge), pellets, pellet_grains, spread_deg)
        }
    };
    let payload_grains = projectile_grains * projectiles as f64;
    let projectile_mass = projectile_grains * GRAIN_KG;
    let payload_mass = payload_grains * GRAIN_KG;
    let speed = c.muzzle_velocity_fps * FOOT_M;
    let projectile_energy = 0.5 * projectile_mass * speed * speed;
    let energy = projectile_energy * projectiles as f64;
    let momentum = payload_mass * speed;

    // Free recoil: the weapon takes the projectiles' momentum plus the powder gases' momentum, less the share a
    // muzzle device turns away. Gas leaves faster than the projectile (gas_velocity_factor).
    let gas_factor = c.gas_velocity_factor.unwrap_or(tuning.gas_velocity_factor);
    let gas_momentum = c.powder_grains * GRAIN_KG * speed * gas_factor;
    let (redirect, added_mass_lb) = match profile.mitigation.muzzle_device {
        MuzzleDevice::None => (0.0, 0.0),
        MuzzleDevice::Compensator { gas_redirect } | MuzzleDevice::Brake { gas_redirect } => (gas_redirect, 0.0),
        MuzzleDevice::Suppressor { gas_redirect, mass_lb } => (gas_redirect, mass_lb),
    };
    let weapon_mass = (profile.mass_lb + added_mass_lb) * POUND_KG;
    let free = |redirected: f64, mass: f64| {
        let impulse = momentum + gas_momentum * (1.0 - redirected);
        let velocity = impulse / mass;
        (impulse, velocity, 0.5 * mass * velocity * velocity / FOOT_POUND_J)
    };
    let (impulse, recoil_velocity, free_recoil) = free(redirect, weapon_mass);
    let felt = free_recoil * (1.0 - profile.mitigation.recoil_pad) * profile.operation_feel();
    let (_, _, unmitigated) = free(0.0, profile.mass_lb * POUND_KG);
    let reduction = if unmitigated > 0.0 { (1.0 - felt / unmitigated).clamp(0.0, 1.0) } else { 0.0 };

    let h = &profile.handling;
    let damage = projectile_energy / 1000.0 * tuning.damage_per_kj * h.damage_scale.unwrap_or(1.0);
    let vertical = tuning.recoil_degrees_per_sqrt_ftlbf * felt.sqrt();
    let horizontal = vertical * h.horizontal_ratio.unwrap_or(tuning.horizontal_ratio);
    let fire_interval = 60.0 / profile.fire_rate_rpm;
    let capacity = profile.capacity();

    let stats = WeaponStats {
        name: profile.name.clone(),
        class: profile.class,
        cartridge: c.name.clone(),
        bore_in,
        projectiles,
        projectile_grains,
        payload_grains,
        projectile_mass_kg: projectile_mass,
        muzzle_velocity_fps: c.muzzle_velocity_fps,
        muzzle_velocity_mps: speed,
        muzzle_energy_ftlbf: energy / FOOT_POUND_J,
        muzzle_energy_j: energy,
        projectile_energy_j: projectile_energy,
        momentum_ns: momentum,
        recoil_impulse_ns: impulse,
        recoil_velocity_fps: recoil_velocity / FOOT_M,
        free_recoil_ftlbf: free_recoil,
        felt_recoil_ftlbf: felt,
        recoil_reduction: reduction,
        fire_rate_rpm: profile.fire_rate_rpm,
        fire_interval_s: fire_interval,
        capacity,
        damage_per_projectile: damage,
        recoil_vertical_degrees: vertical,
        recoil_horizontal_degrees: horizontal,
    };

    // Reloads either swap a full magazine or speedloader for the loaded one, or top up one round at a time
    // from a reserve of loose rounds.
    let (reload_kind, reload_seconds, spares) = match &profile.feed {
        Feed::Cylinder { rounds, loader: Loader::Speedloader { spares, reload_seconds } }
        | Feed::DetachableMagazine { capacity: rounds, spares, reload_seconds } => {
            ("swap", *reload_seconds, vec![*rounds; *spares as usize])
        }
        Feed::Cylinder { loader: Loader::Loose { reserve, round_seconds }, .. }
        | Feed::TubeMagazine { reserve, round_seconds, .. }
        | Feed::InternalMagazine { reserve, round_seconds, .. } => ("top_up", *round_seconds, vec![*reserve]),
    };
    let modes: Vec<&str> = profile
        .fire_modes
        .iter()
        .map(|m| match m {
            FireMode::Single => "single",
            FireMode::Burst { .. } => "burst",
            FireMode::Auto => "auto",
        })
        .collect();
    let burst_rounds = profile.fire_modes.iter().find_map(|m| match m {
        FireMode::Burst { rounds } => Some(*rounds),
        _ => None,
    });
    let mut config = json!({
        "magazine_capacity": capacity,
        "initial_loaded": capacity,
        "initial_spares": spares,
        "fire_interval": fire_interval,
        "reload_kind": reload_kind,
        "reload_seconds": reload_seconds,
        "modes": modes,
    });
    if let Some(rounds) = burst_rounds {
        config["burst_rounds"] = json!(rounds);
    }
    let components = BTreeMap::from([
        ("WeaponConfig".to_owned(), config),
        (
            "Ballistics".to_owned(),
            json!({
                "speed_mps": speed,
                "damage": damage,
                "projectiles": projectiles,
                "spread_deg": spread_deg,
                "projectile_mass_kg": projectile_mass,
            }),
        ),
        (
            "WeaponRecoil".to_owned(),
            json!({
                "vertical_degrees": vertical,
                "horizontal_degrees": horizontal,
                "rise_seconds": h.rise_seconds.unwrap_or(tuning.rise_seconds),
            }),
        ),
        ("WeaponStats".to_owned(), serde_json::to_value(&stats).map_err(|e| e.to_string())?),
    ]);
    Ok(ResolvedWeapon { stats, components })
}

/// Reads the scene's `weapon_profiles` and `weapon_tuning` resources (either may be absent).
pub fn scene_profiles(
    resources: &BTreeMap<String, Value>,
) -> Result<(BTreeMap<String, WeaponProfile>, WeaponTuning), String> {
    let profiles = match resources.get("weapon_profiles") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(value) => serde_json::from_value::<BTreeMap<String, WeaponProfile>>(value.clone())
            .map_err(|e| format!("weapon_profiles: {e}"))?,
    };
    let tuning = match resources.get("weapon_tuning") {
        None | Some(Value::Null) => WeaponTuning::default(),
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|e| format!("weapon_tuning: {e}"))?
        }
    };
    for (id, profile) in &profiles {
        profile.validate().map_err(|e| format!("weapon profile `{id}`: {e}"))?;
    }
    tuning.validate()?;
    Ok((profiles, tuning))
}

/// The profile id a `WeaponProfile` component selects.
pub fn selected_profile(component: &Value) -> Result<&str, String> {
    component
        .get("profile")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "WeaponProfile needs a `profile` id".to_owned())
}

/// The first-person footage manifest asset for a scene: the `footage` of the first entity's selected profile that
/// names one, else `fallback`.
pub fn footage_asset<'a>(manifest: &'a crate::SceneManifest, fallback: &'a str) -> &'a str {
    let profiles = manifest.resources.values.get("weapon_profiles");
    manifest
        .entities
        .iter()
        .filter_map(|entity| entity.components.get("WeaponProfile"))
        .filter_map(|selection| selected_profile(selection).ok())
        .find_map(|id| profiles?.get(id)?.get("footage")?.as_str())
        .unwrap_or(fallback)
}

/// Merges `patch`'s top-level fields over `base` (an object), keeping every authored field the patch does not set.
pub fn merge_fields(base: Option<&Value>, patch: &Value) -> Value {
    let mut merged = base.and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(fields) = patch.as_object() {
        for (key, value) in fields {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(value: Value) -> WeaponProfile {
        serde_json::from_value(value).unwrap()
    }
    fn magnum() -> WeaponProfile {
        profile(json!({
            "name": "X-frame .500 revolver",
            "class": "revolver",
            "cartridge": {
                "name": ".500 S&W Magnum 350 gr",
                "projectile": {"kind": "bullet", "caliber_in": 0.5, "grains": 350.0},
                "powder_grains": 42.0,
                "muzzle_velocity_fps": 1900.0
            },
            "barrel_in": 8.375,
            "mass_lb": 4.5,
            "action": {"kind": "revolver", "double_action": true},
            "feed": {"kind": "cylinder", "rounds": 5,
                     "loader": {"kind": "speedloader", "spares": 3, "reload_seconds": 3.2}},
            "fire_modes": [{"kind": "single"}],
            "fire_rate_rpm": 120.0,
            "mitigation": {"muzzle_device": {"kind": "compensator", "gas_redirect": 0.25}}
        }))
    }

    #[test]
    fn a_magnum_revolver_matches_published_ballistics() {
        let r = resolve(&magnum(), &WeaponTuning::default()).unwrap();
        let s = &r.stats;
        // 350 gr at 1900 ft/s: 350 * 1900^2 / 450,437 = 2805 ft·lbf.
        assert!((s.muzzle_energy_ftlbf - 2805.0).abs() < 2.0, "{}", s.muzzle_energy_ftlbf);
        assert!((s.muzzle_energy_j - 2805.0 * FOOT_POUND_J).abs() < 3.0);
        assert!((s.muzzle_velocity_mps - 579.12).abs() < 0.01);
        // A 4.5 lb gun: free recoil in the 40s of ft·lbf with a compensator, and more without it.
        assert!((35.0..50.0).contains(&s.free_recoil_ftlbf), "{}", s.free_recoil_ftlbf);
        let mut bare = magnum();
        bare.mitigation = Mitigation::default();
        let b = resolve(&bare, &WeaponTuning::default()).unwrap().stats;
        assert!(b.free_recoil_ftlbf > s.free_recoil_ftlbf && b.recoil_reduction == 0.0);
        assert!(s.recoil_reduction > 0.05);
        assert_eq!((s.capacity, s.projectiles), (5, 1));
        assert!((s.fire_interval_s - 0.5).abs() < 1e-9);
        let config = &r.components["WeaponConfig"];
        assert_eq!(config["magazine_capacity"], 5);
        assert_eq!(config["initial_spares"], json!([5, 5, 5]));
        assert_eq!(config["reload_kind"], "swap");
        assert_eq!(config["modes"], json!(["single"]));
        assert!(config.get("burst_rounds").is_none());
    }

    #[test]
    fn a_rifle_carbine_reproduces_the_previous_tuned_feel() {
        let carbine = profile(json!({
            "name": "5.56 carbine", "class": "carbine",
            "cartridge": {"name": "5.56x45 62 gr",
                          "projectile": {"kind": "bullet", "caliber_in": 0.224, "grains": 62.0},
                          "powder_grains": 25.0, "muzzle_velocity_fps": 2900.0},
            "barrel_in": 14.5, "mass_lb": 7.5,
            "action": {"kind": "select_fire", "operation": "gas"},
            "feed": {"kind": "detachable_magazine", "capacity": 30, "spares": 3, "reload_seconds": 2.6},
            "fire_modes": [{"kind": "single"}, {"kind": "burst", "rounds": 3}, {"kind": "auto"}],
            "fire_rate_rpm": 750.0
        }));
        let r = resolve(&carbine, &WeaponTuning::default()).unwrap();
        // About 4 ft·lbf of free recoil for an AR-pattern carbine; the tuning maps it near the old 0.55 degrees.
        assert!((3.0..5.0).contains(&r.stats.free_recoil_ftlbf), "{}", r.stats.free_recoil_ftlbf);
        assert!((r.stats.recoil_vertical_degrees - 0.55).abs() < 0.08, "{}", r.stats.recoil_vertical_degrees);
        assert!((r.stats.damage_per_projectile - 35.0).abs() < 3.0, "{}", r.stats.damage_per_projectile);
        assert!((r.stats.fire_interval_s - 0.08).abs() < 1e-9);
        assert_eq!(r.components["WeaponConfig"]["modes"], json!(["single", "burst", "auto"]));
        assert_eq!(r.components["WeaponConfig"]["burst_rounds"], 3);
    }

    #[test]
    fn a_shotgun_is_described_by_gauge_and_pellets() {
        assert!((gauge_bore_in(12.0) - 0.729).abs() < 0.002);
        assert!((gauge_bore_in(20.0) - 0.615).abs() < 0.002);
        let pump = profile(json!({
            "name": "12 gauge pump", "class": "shotgun",
            "cartridge": {"name": "12 ga 2-3/4\" 00 buck",
                          "projectile": {"kind": "shot", "gauge": 12.0, "pellets": 9, "pellet_grains": 53.8,
                                         "spread_deg": 3.0},
                          "powder_grains": 28.0, "muzzle_velocity_fps": 1325.0},
            "barrel_in": 18.5, "mass_lb": 7.5,
            "action": {"kind": "pump"},
            "feed": {"kind": "tube_magazine", "capacity": 5, "reserve": 20, "round_seconds": 0.55},
            "fire_modes": [{"kind": "single"}],
            "fire_rate_rpm": 70.0,
            "mitigation": {"recoil_pad": 0.25}
        }));
        let r = resolve(&pump, &WeaponTuning::default()).unwrap();
        assert_eq!(r.stats.projectiles, 9);
        assert!((r.stats.payload_grains - 484.2).abs() < 0.01);
        assert!((r.stats.bore_in - 0.729).abs() < 0.002);
        // 00 buck from a 7.5 lb pump: roughly 20-30 ft·lbf free recoil; the pad takes a quarter off what's felt.
        assert!((18.0..32.0).contains(&r.stats.free_recoil_ftlbf), "{}", r.stats.free_recoil_ftlbf);
        assert!((r.stats.felt_recoil_ftlbf - r.stats.free_recoil_ftlbf * 0.75).abs() < 1e-9);
        assert_eq!(r.components["Ballistics"]["projectiles"], 9);
        assert_eq!(r.components["Ballistics"]["spread_deg"], 3.0);
        assert_eq!(r.components["WeaponConfig"]["reload_kind"], "top_up");
        assert_eq!(r.components["WeaponConfig"]["initial_spares"], json!([20]));
    }

    #[test]
    fn invalid_profiles_are_rejected_with_a_reason() {
        let mut p = magnum();
        p.fire_modes = vec![FireMode::Auto];
        assert!(p.validate().unwrap_err().contains("select-fire"));
        let mut p = magnum();
        p.cartridge.muzzle_velocity_fps = f64::NAN;
        assert!(p.validate().is_err());
        let mut p = magnum();
        p.action = Action::Pump;
        assert!(p.validate().unwrap_err().contains("cylinder"));
        let unknown = json!({"name": "x", "class": "revolver", "laser": true});
        assert!(serde_json::from_value::<WeaponProfile>(unknown).is_err());
        let resources = BTreeMap::from([("weapon_profiles".to_owned(), json!({"bad": {"name": "x"}}))]);
        assert!(scene_profiles(&resources).unwrap_err().contains("weapon_profiles"));
    }

    #[test]
    fn merging_keeps_authored_fields_the_profile_does_not_set() {
        let base = json!({"speed_mps": 715.0, "gravity_mps2": 9.81, "show_tracers": true});
        let merged = merge_fields(Some(&base), &json!({"speed_mps": 579.0, "projectiles": 1}));
        assert_eq!(merged, json!({"speed_mps": 579.0, "gravity_mps2": 9.81, "show_tracers": true, "projectiles": 1}));
    }
}
