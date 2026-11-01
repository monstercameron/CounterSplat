// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/tonemapping/tonemappingLinear.js
fn toneMap(color: vec3f) -> vec3f {
    return color * getExposure();
}
