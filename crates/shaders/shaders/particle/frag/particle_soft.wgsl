// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_soft.js
    var depth: f32 = getLinearScreenDepthFrag();
    var particleDepth: f32 = vDepth;
    var depthDiff: f32 = saturate(abs(particleDepth - depth) * uniform.softening);
    a = a * depthDiff;
