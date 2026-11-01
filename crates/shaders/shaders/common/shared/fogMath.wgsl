// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/shared/fogMath.js
fn evaluateFogFactorLinear(depth: f32, fogStart: f32, fogEnd: f32) -> f32 {
    return clamp((fogEnd - depth) / (fogEnd - fogStart), 0.0, 1.0);
}

fn evaluateFogFactorExp(depth: f32, fogDensity: f32) -> f32 {
    return clamp(exp(-depth * fogDensity), 0.0, 1.0);
}

fn evaluateFogFactorExp2(depth: f32, fogDensity: f32) -> f32 {
    return clamp(exp(-depth * depth * fogDensity * fogDensity), 0.0, 1.0);
}
