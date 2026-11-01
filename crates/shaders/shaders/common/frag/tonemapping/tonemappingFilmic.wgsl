// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/tonemapping/tonemappingFilmic.js
const A: f32 = 0.15;
const B: f32 = 0.50;
const C: f32 = 0.10;
const D: f32 = 0.20;
const E: f32 = 0.02;
const F: f32 = 0.30;
const W: f32 = 11.2;

fn uncharted2Tonemap(x: vec3f) -> vec3f {
    return ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - vec3f(E / F);
}

fn toneMap(color: vec3f) -> vec3f {
    var c: vec3f = uncharted2Tonemap(color * getExposure());
    let whiteScale: vec3f = vec3f(1.0) / uncharted2Tonemap(vec3f(W, W, W));
    c *= whiteScale;
    return c;
}
