// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_wrap.js
    let origParticlePos: vec3f = particlePos;
    particlePos = particlePos - uniform.matrix_model[3].xyz;
    particlePos = (particlePos % uniform.wrapBounds) - uniform.wrapBounds * 0.5;
    particlePos = particlePos + uniform.matrix_model[3].xyz;
    particlePosMoved = particlePos - origParticlePos;
