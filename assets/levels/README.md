# CounterSplat development splats

Each scene is stored under its SuperSplat scene ID. The streamed scenes contain the original `lod-meta.json`,
all referenced chunk metadata and WebP textures, the public viewer's camera settings, and collision files when
advertised. `source.json` records the source and creator. `download-inventory.json` records file sizes and SHA-256
checksums for the downloaded resources.

`58d333ae` is a single unpacked SOG; the public viewer does not publish a separate LOD manifest for it. The other
ten use streamed SOG with 3–8 published LOD levels. Four are the levels referenced by the Neural-Sight project.
`catalog.json` maps names, creators, and optional `preview.webp` thumbnails to their local manifests. The preview images are downloaded development assets and remain ignored with their levels. To resume downloads or verify them offline, run from
CounterSplat's project root:

```powershell
python scripts/download_supersplat.py
python scripts/download_supersplat.py --verify-only
python scripts/download_previews.py
```

The downloaded scene directories are excluded from version control; this README and the level catalog are tracked. Scene files remain subject to the applicable creator
permissions and licenses. The greenhouse page identifies its license as CC BY 4.0; see its `source.json` for credit.

A catalog entry may include `"player_units_per_meter": 1.2` to calibrate the player's initial
size and movement against that scan. The allowed range is 0.5–2.5; omitted values use the
gameplay scene default. This calibrates the player rig rather than changing the scan geometry.
Users can adjust and save their own per-level value through **Player controls**.
