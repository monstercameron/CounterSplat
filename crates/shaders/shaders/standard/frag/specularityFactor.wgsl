// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/specularityFactor.js
#ifdef STD_SPECULARITYFACTOR_CONSTANT
    uniform material_specularityFactor: f32;
#endif

fn getSpecularityFactor() {
    var specularityFactor = 1.0;

    #ifdef STD_SPECULARITYFACTOR_CONSTANT
    specularityFactor = specularityFactor * uniform.material_specularityFactor;
    #endif

    #ifdef STD_SPECULARITYFACTOR_TEXTURE
    specularityFactor = specularityFactor * textureSampleBias({STD_SPECULARITYFACTOR_TEXTURE_NAME}, {STD_SPECULARITYFACTOR_TEXTURE_NAME}Sampler, {STD_SPECULARITYFACTOR_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_SPECULARITYFACTOR_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_SPECULARITYFACTOR_VERTEX
    specularityFactor = specularityFactor * saturate(vVertexColor.{STD_SPECULARITYFACTOR_VERTEX_CHANNEL});
    #endif

    dSpecularityFactor = specularityFactor;
}
