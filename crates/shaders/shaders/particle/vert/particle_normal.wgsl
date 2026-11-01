// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_normal.js
output.Normal = normalize(localPos + uniform.matrix_viewInverse[2].xyz);
