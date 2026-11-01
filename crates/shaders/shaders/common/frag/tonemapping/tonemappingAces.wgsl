// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/tonemapping/tonemappingAces.js
fn toneMap(color: vec3f) -> vec3f {
    let tA: f32 = 2.51;
    let tB: f32 = 0.03;
    let tC: f32 = 2.43;
    let tD: f32 = 0.59;
    let tE: f32 = 0.14;
    let x: vec3f = color * getExposure();
    return (x * (tA * x + tB)) / (x * (tC * x + tD) + tE);
}
