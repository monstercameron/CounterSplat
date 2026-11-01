// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_lambert.js
    var negNormal: vec3f = max(normal, vec3(0.0));
    var posNormal: vec3f = max(-normal, vec3(0.0));
