# PlayCanvas shader notice

The WGSL files in `shaders/` are from the PlayCanvas engine, commit
`209ce5752527ab5050ec9632ca6dc1272542b70d`, copyright PlayCanvas Ltd and contributors. They are MIT licensed; the
full license is in [../../third_party/playcanvas/LICENSE](../../third_party/playcanvas/LICENSE). Individual source
headers retain their original paths. `chunk_map.json` maps PlayCanvas include keys to those files.

CounterSplat's Rust composition code and local scene modifier adapt those chunks to this workspace.
