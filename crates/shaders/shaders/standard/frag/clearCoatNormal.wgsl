// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/clearCoatNormal.js
#ifdef STD_CLEARCOATNORMAL_TEXTURE
    uniform material_clearCoatBumpiness: f32;
#endif

fn getClearCoatNormal() {
#ifdef STD_CLEARCOATNORMAL_TEXTURE
    var normalMap: vec3f = {STD_CLEARCOATNORMAL_TEXTURE_DECODE}(textureSampleBias({STD_CLEARCOATNORMAL_TEXTURE_NAME}, {STD_CLEARCOATNORMAL_TEXTURE_NAME}Sampler, {STD_CLEARCOATNORMAL_TEXTURE_UV}, {STD_TEXTURE_BIAS}));
    normalMap = mix(vec3f(0.0, 0.0, 1.0), normalMap, uniform.material_clearCoatBumpiness);
    ccNormalW = normalize(dTBN * normalMap);
#else
    ccNormalW = dVertexNormalW;
#endif
}
