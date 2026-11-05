# Weapon profiles

Every gun is described by one data model, `countersplat_scene::weapon_profile`. A profile is a real-world
weapon in its makers' units, built from polymorphic parts, so a new gun is new data rather than new code:

| Part | Variants |
| --- | --- |
| `cartridge.projectile` | `bullet` (`caliber_in`, `grains`), `slug` (`gauge`, `grains`), `shot` (`gauge`, `pellets`, `pellet_grains`, `spread_deg`) |
| `action` | `revolver` (`double_action`), `semi_automatic` / `select_fire` (`operation`: `gas`, `short_recoil`, `blowback`, `delayed_blowback`), `pump`, `bolt`, `lever`, `break_action` |
| `feed` | `cylinder` (`rounds`, `loader`: `speedloader` or `loose`), `detachable_magazine`, `tube_magazine`, `internal_magazine` |
| `fire_modes` | `single`, `burst` (`rounds`), `auto`; burst and auto need a `select_fire` action |
| `mitigation` | `muzzle_device` (`none`, `compensator`, `brake`, `suppressor`, each with a `gas_redirect` fraction) and `recoil_pad` |

A profile also holds `cartridge.powder_grains` and `muzzle_velocity_fps`, plus `barrel_in`, the loaded
`mass_lb`, `fire_rate_rpm`, optional `handling` overrides and an optional `footage` (the first-person manifest asset
that draws it). Unknown fields and out-of-range values are rejected with the reason.

## Physics

`resolve` applies the same formulas to every profile:

- Muzzle energy `½mv²` (reported in ft·lbf and J): 350 gr at 1900 ft/s is 2805 ft·lbf.
- Momentum of the payload, `mv`.
- Free recoil: the weapon takes the payload's momentum plus the powder gases' momentum (gas leaving at
  `gas_velocity_factor` times muzzle velocity, 1.75 by default), less the share a muzzle device redirects. Recoil
  velocity is that impulse over the weapon's mass (plus a suppressor's), and free recoil energy is `½MV²`.
- Felt recoil: free recoil less the recoil pad, and less what a self-loading operation spreads out (gas 20%,
  short recoil and delayed blowback 10%, blowback 5%).
- A gauge's bore is the diameter of a lead ball of 1/gauge pound (12 ga = 0.729 in).

`weapon_tuning` turns physics into game values, the same way for every gun: damage per kilojoule of each
projectile's energy, and view kick in degrees per square root of felt ft·lbf. The defaults reproduce the earlier
hand-tuned 5.56 carbine (0.55 degrees of kick, 35 damage).

## In a scene

`gameplay.json` keeps profiles in the `weapon_profiles` resource and the constants in `weapon_tuning`. An entity
picks one with `WeaponProfile: {"profile": "<id>"}`. When the scene is instantiated, the profile's values are
merged over that entity's authored components; fields a profile does not set (gravity, range, tracer display and
so on) keep their authored values:

- `WeaponConfig`: capacity, spares, fire interval, `reload_kind` (`swap` for magazines and speedloaders, `top_up`
  for loose rounds loaded one at a time), reload seconds (per round for `top_up`), `modes`, `burst_rounds`.
- `Ballistics`: speed, damage per projectile, projectile count, spread and mass.
- `WeaponRecoil`: vertical and horizontal kick, rise time.
- `WeaponStats`: the full physics, readable in the entity inspector.

The `weapon_stats` resource publishes every profile's physics. Projectiles carry their mass, so an impact's
momentum is real: the can range pushes a hit can with `momentum_transfer` of the bullet's momentum.

The shipped scene selects `revolver_500`; it also defines a .44 Magnum revolver (loose rounds), a 9mm pistol, a
5.56 select-fire carbine, a 12 gauge pump shotgun (9 pellets) and a .308 bolt rifle with a brake. Print them all:

```bash
cargo run --release -p countersplat-scene --example weapon_stats -- assets/scenes/gameplay.json
```
