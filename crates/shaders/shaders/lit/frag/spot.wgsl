// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/spot.js
fn getSpotEffect(lightSpotDir: vec3f, lightInnerConeAngle: f32, lightOuterConeAngle: f32, lightDirNorm: vec3f) -> f32 {
    let cosAngle: f32 = dot(lightDirNorm, lightSpotDir);
    return smoothstep(lightOuterConeAngle, lightInnerConeAngle, cosAngle);
}