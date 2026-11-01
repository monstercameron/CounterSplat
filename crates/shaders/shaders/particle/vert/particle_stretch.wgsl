// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_stretch.js
    let moveDir: vec3f = inVel * uniform.stretch;
    var posPrev: vec3f = particlePos - moveDir;
    posPrev = posPrev + particlePosMoved;

    let viewRotationTemp: mat3x3f = mat3x3f(uniform.matrix_view[0].xyz, uniform.matrix_view[1].xyz, uniform.matrix_view[2].xyz);
    let centerToVertexV: vec2f = normalize((viewRotationTemp * localPos).xy);

    let interpolation: f32 = dot(-velocityV, centerToVertexV) * 0.5 + 0.5;

    particlePos = mix(particlePos, posPrev, interpolation);
