// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_normalMap.js
    let sampledNormal: vec4f = textureSample(normalMap, normalMapSampler, vec2f(input.texCoordsAlphaLife.x, 1.0 - input.texCoordsAlphaLife.y));
    let normalMap: vec3f = normalize(sampledNormal.xyz * 2.0 - 1.0);

    let ParticleMat = mat3x3<f32>(ParticleMat0, ParticleMat1, ParticleMat2);
    let normal: vec3f = ParticleMat * normalMap;
