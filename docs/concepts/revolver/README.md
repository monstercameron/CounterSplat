# Revolver view-model concepts

Concept stills for a first-person .500 Magnum revolver view model, one per
pose of the weapon state machine (`assets/scripts/first_person_controller.lua`).
Each sits on flat chroma green (#00FF00), which the weapon compositor keys out.
They are stills, not playback footage; the game does not load them.

## Body-cam set (current direction)

The camera is a wide-angle (about 100-110 degrees) lens at the centre of the
chest, level, so poses above the chest are seen from below and low poses from
above.

| File | State | Notes |
| --- | --- | --- |
| `revolver_bodycam_low_ready.png` | `lowReady` | Style reference for the set. Thumbs not crossed as intended. |
| `revolver_ads_sight_picture.png` | `ads` (in game) | Straight down the sights; chosen over the body-cam ADS. Thumbs run forward along the frame. |
| `revolver_bodycam_ads.png` | not used | Revolver drawn side-on, muzzle pointing left instead of ahead. |
| `revolver_ads_kick_center.png` | peak of `fireADS` (in game) | Centred sight picture with the muzzle tipped up. Moved down 18% (`footage_still ... shift=0.18`) it is also the hip recoil peak (`fireHip`). |
| (derived) | hip (in game) | `revolver_ads_sight_picture.png` moved down 18% with `footage_still ... shift=0.18`. The body-cam hip and hip-recoil stills are not used. |
| `revolver_bodycam_hip.png` | hip phase of `raising` | Side-on, muzzle left. |
| `revolver_bodycam_fire_hip_kick.png` | peak of `fireHip` | Side-on, muzzle up and left. |
| `revolver_bodycam_fire_ads_kick.png` | peak of `fireADS` | |
| `revolver_bodycam_high_ready.png` | `runForward` | |
| `revolver_bodycam_run_lateral.png` | `runLeft` | |
| `revolver_bodycam_run_right.png` | `runRight` | Barrel still leans left; only subtly different from `runForward`. |
| `revolver_bodycam_run_backward.png` | `runBackward` | |
| `revolver_bodycam_slide.png` | `slide` | |
| `revolver_bodycam_observe.png` | inspect (no state yet) | Only about 25 degrees up. |
| `revolver_bodycam_reload_close.png` | end of `reload` | Muzzle up-left rather than down. |

Not made (refused by the image tool's filter on every attempt): the reload
stages open, eject and load. Crouch states
reuse the standing poses because crouching only lowers the camera.

## Game-style set (earlier, eye-level camera)

| File | State |
| --- | --- |
| `revolver_low_ready.png` | `lowReady` |
| `revolver_ads.png` | `ads` (hands approximate) |
| `revolver_hip.png` | hip phase (hands approximate) |
| `revolver_fire_ads_kick.png` | peak of `fireADS` |
| `revolver_high_ready.png` | `run` |
| `revolver_run_lateral.png` | `runLateral` |
| `revolver_observe.png` | inspect |
| `revolver_reload_open.png` | reload, cylinder open |

## How they were made

Generated on 2026-11-05 with the OpenAI Codex CLI (`codex exec`, image
generation tool). Each `prompt_*.txt` holds the exact prompt. Each prompt
attached earlier stills as references, so the revolver, gloves and lighting
match. The two-handed poses also attached a photo of a two-handed revolver grip
as a hand reference; that photo is not ours and is not in the repo. All are
1672x941 PNGs.
