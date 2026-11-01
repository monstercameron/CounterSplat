// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/falloffLinear.js
fn getFalloffLinear(lightRadius: f32, lightDir: vec3f) -> f32 {
    let d: f32 = length(lightDir);
    return max(((lightRadius - d) / lightRadius), 0.0);
}
