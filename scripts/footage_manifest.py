"""Writes manifest.json's frame lists: revolver concept stills, plus Seedance 2.5 footage where it exists
(see scripts/seedance_frames.py). Run: python scripts/footage_manifest.py assets/images/first-person/playback/manifest.json

Lua keeps its frame counts (gameplay.json `first_person_clips`); Rust clamps the requested index to each list,
so each list is laid out so the pose changes at the right phase.
"""
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
manifest = json.loads(path.read_text(encoding="utf-8"))

def frame(n):
    return f"first-person-frame-{n}"

LOW, HIP, ADS, RUN = frame("0612"), frame("0636"), frame("0644"), frame("0935")
OPEN, CLOSE, HIP_SHOT, ADS_SHOT = frame("0645"), frame("0646"), frame("0483"), frame("0477")
INSPECT = frame("inspect")
RUN_LEFT, RUN_RIGHT, RUN_BACK = frame("run-left"), frame("run-right"), frame("run-backward")

def run(first, count):
    return [frame("%04d" % (first + i)) for i in range(count)]

# Seedance 2.5 footage: raising/0002-0025 animate low ready -> hip (0025 is the hip pose), raising/0026-0032 hip ->
# ADS (0033 is the ADS sight picture), and reload/0001-0084 the cylinder open -> eject -> speedloader -> close.
# runFromLow/0002-0018 and runFromADS/0002-0018 carry low ready and ADS into the one-handed sprint carry; their
# first and last indices are the shared endpoint stills, so the carries join the poses without a crossfade.
RAISE = run(612, 33)
RELOAD_ACTION = run(645, 84)
LOW_TO_RUN = run(953, 17)
ADS_TO_RUN = run(936, 17)

def named(clip, count):
    """Footage frames declared by name: first-person-frame-<clip>-01.. (scene asset playback/<folder>/0001..)."""
    return [frame("%s-%02d" % (clip, i + 1)) for i in range(count)]

# The in-between frames of each gap clip; their endpoint stills come from the poses they join.
EMPTY = frame("empty")  # the holster's end: nothing in hand
INSPECT_IN = [LOW] + named("inspect-in", 11) + [INSPECT]

lists = {
    # One clip, three poses read by index: low ready 1, hip 25 (phase 24/32), ADS 33.
    "raising": RAISE,
    "lowering": [ADS] * 13 + [LOW] * 14,
    "lowReady": [LOW],
    "run": [RUN],
    "runLateral": [RUN, RUN],
    # One sprint loop per direction (the run state machine picks one from the movement keys).
    "runForward": [RUN, RUN],
    "runLeft": [RUN_LEFT, RUN_LEFT],
    "runRight": [RUN_RIGHT, RUN_RIGHT],
    "runBackward": [RUN_BACK, RUN_BACK],
    "runFromLow": [LOW] + LOW_TO_RUN + [RUN],
    "runFromADS": [ADS] + ADS_TO_RUN + [RUN],
    # Open the cylinder from the held pose, the filmed reload, then close back into the same pose. From low ready
    # the reload ends on its closed frame, which is near low ready: the clip change back crossfades the rest
    # (generated "settle into low ready" takes swung the gun up or reopened the cylinder).
    "reload": [LOW] + named("reload-in-low", 15) + RELOAD_ACTION,
    "reloadADS": [ADS] + named("reload-in-ads", 15) + RELOAD_ACTION + named("reload-out-ads", 15) + [ADS],
    "crouchEnterLow": [LOW], "crouchExitLow": [LOW], "crouchIdleLow": [LOW], "crouchWalkLow": [LOW],
    "crouchEnterADS": [ADS], "crouchExitADS": [ADS], "crouchIdleADS": [ADS], "crouchWalkADS": [ADS],
    # Indices 1-2 are the shot (the host holds them as the muzzle-flash frames); the rest settle on the pose.
    # Hip fire holds the hip pose and kicks it procedurally (gameplay.json disables the fireHip clip): generated
    # hip-shot footage lunged the barrel out of the frame.
    "fireHip": [HIP] * 7,
    "fireADS": [ADS_SHOT] * 2 + [ADS] * 5,
    # inspectEnter plays 0 -> .5 (low ready -> observe pose), inspecting holds .5, inspectExit retraces to 1.
    "inspect": INSPECT_IN + INSPECT_IN[-2::-1],
    # The drop into the slide plays, then holds its last frame; slideExit gets up into low ready.
    "slide": [RUN] + named("slide-in", 11) + [frame("slide")],
    "slideExit": [frame("slide")] + named("slide-out", 11) + [LOW],
    # Stow plays low ready -> holstered (out of frame); equip plays it backwards.
    "holster": [LOW] + named("holster", 15) + [EMPTY],
}

NOTE = ("Placeholder: revolver concept stills (docs/concepts/revolver) stand in for footage; "
        "each list repeats stills so poses change at the right phase.")
for name, frames in lists.items():
    entry = manifest.get(name) or {"state": name, "resolution": [1280, 720]}
    entry["frames"] = frames
    entry.pop("sourceIndices", None)
    entry["note"] = NOTE
    manifest[name] = entry
missing = sorted(set(manifest) - set(lists))
if missing:
    sys.exit(f"clips without a still layout: {missing}")
path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
print({name: len(frames) for name, frames in lists.items()})
