// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_lighting.js
    let light: vec3f = negNormal.x * uniform.lightCube[0] + posNormal.x * uniform.lightCube[1] +
                       negNormal.y * uniform.lightCube[2] + posNormal.y * uniform.lightCube[3] +
                       negNormal.z * uniform.lightCube[4] + posNormal.z * uniform.lightCube[5];

    rgb = rgb * light;
