# Pursuit mode

In the browser canvas, load a level, press **Esc**, and select **Game mode → Pursuit**.
Start or restart from this page; **Reset status** ends the run, clears targets and
scores, and restores health, armor, and ammunition. The existing practice targets
are restored when the mode is reset. A defeated player can open the same page
and choose **Play again**.

Targets spawn continuously up to the configured cap, chase the player's position,
and damage health and armor when in range. When voxel collision is available,
spawns are checked against the ground and capsule clearance, movement is resolved
by the collision worker, and walls block attacks. If no level collider exists,
the mode still runs without those level-collision checks.

Each rifle shot uses the weapon's existing Ballistics damage (35 by default).
Targets have 100 health by default, and each body zone has a separate 100-point
damage indicator. Shooting any zone reduces target health; its own damage indicator
changes that zone from gold toward red. The HUD reports the last shot's damage,
hit zone, remaining zone health, target health, kills, and player health.

The scene's `pursuit_mode` entity owns `PursuitConfig`. In the ECS inspector,
the script exports controls for the spawn interval, maximum count, spawn distance,
pursuit speed, target and zone health, and attack damage, rate, and reach.
Shot damage remains a weapon setting on `first_person.Ballistics`. Existing
targets retain the zone health assigned at spawn; reset or restart to apply a
new health setting to every target.

Gameplay rules live in `assets/scripts/pursuit_mode.lua`; the weapon, health,
hitbox and physics interfaces remain reusable. Menu actions relay the typed
`game.mode_request` event to the gameplay Lua world. `game.mode_state` publishes
status and counters for the menu and other scripts. Lua emits compact colored
hitbox snapshots; the browser expands them into world-space lines for the GPU.
