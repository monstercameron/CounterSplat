// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/clearCoatGloss.js
    uniform material_clearCoatGloss: f32;

fn getClearCoatGlossiness() {
    ccGlossiness = uniform.material_clearCoatGloss;

    #ifdef STD_CLEARCOATGLOSS_TEXTURE
    ccGlossiness = ccGlossiness * textureSampleBias({STD_CLEARCOATGLOSS_TEXTURE_NAME}, {STD_CLEARCOATGLOSS_TEXTURE_NAME}Sampler, {STD_CLEARCOATGLOSS_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_CLEARCOATGLOSS_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_CLEARCOATGLOSS_VERTEX
    ccGlossiness = ccGlossiness * saturate(vVertexColor.{STD_CLEARCOATGLOSS_VERTEX_CHANNEL});
    #endif

    #ifdef STD_CLEARCOATGLOSS_INVERT
    ccGlossiness = 1.0 - ccGlossiness;
    #endif

    ccGlossiness += 0.0000001;
}
