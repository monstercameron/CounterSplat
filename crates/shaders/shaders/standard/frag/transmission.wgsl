// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/transmission.js
#ifdef STD_REFRACTION_CONSTANT
    uniform material_refraction: f32;
#endif

fn getRefraction() {
    var refraction: f32 = 1.0;

    #ifdef STD_REFRACTION_CONSTANT
    refraction = uniform.material_refraction;
    #endif

    #ifdef STD_REFRACTION_TEXTURE
    refraction = refraction * textureSampleBias({STD_REFRACTION_TEXTURE_NAME}, {STD_REFRACTION_TEXTURE_NAME}Sampler, {STD_REFRACTION_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_REFRACTION_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_REFRACTION_VERTEX
    refraction = refraction * saturate(vVertexColor.{STD_REFRACTION_VERTEX_CHANNEL});
    #endif

    dTransmission = refraction;
}
