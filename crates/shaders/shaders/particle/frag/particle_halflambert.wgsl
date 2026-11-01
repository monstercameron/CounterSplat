// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_halflambert.js
    var negNormal: vec3f = normal * 0.5 + 0.5;
    var posNormal: vec3f = -normal * 0.5 + 0.5;
    negNormal = negNormal * negNormal;
    posNormal = posNormal * posNormal;
