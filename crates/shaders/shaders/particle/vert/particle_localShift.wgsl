// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_localShift.js
particlePos = (uniform.matrix_model * vec4f(particlePos, 1.0)).xyz;
