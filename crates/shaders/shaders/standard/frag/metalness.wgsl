// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/metalness.js
#ifdef STD_METALNESS_CONSTANT
uniform material_metalness: f32;
#endif

fn getMetalness() {
    var metalness: f32 = 1.0;

    #ifdef STD_METALNESS_CONSTANT
        metalness = metalness * uniform.material_metalness;
    #endif

    #ifdef STD_METALNESS_TEXTURE
        metalness = metalness * textureSampleBias({STD_METALNESS_TEXTURE_NAME}, {STD_METALNESS_TEXTURE_NAME}Sampler, {STD_METALNESS_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_METALNESS_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_METALNESS_VERTEX
    metalness = metalness * saturate(vVertexColor.{STD_METALNESS_VERTEX_CHANNEL});
    #endif

    dMetalness = metalness;
}
