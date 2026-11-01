// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/sheenGloss.js
uniform material_sheenGloss: f32;

fn getSheenGlossiness() {
    var sheenGlossiness = uniform.material_sheenGloss;

    #ifdef STD_SHEENGLOSS_TEXTURE
    sheenGlossiness = sheenGlossiness * textureSampleBias({STD_SHEENGLOSS_TEXTURE_NAME}, {STD_SHEENGLOSS_TEXTURE_NAME}Sampler, {STD_SHEENGLOSS_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_SHEENGLOSS_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_SHEENGLOSS_VERTEX
    sheenGlossiness = sheenGlossiness * saturate(vVertexColor.{STD_SHEENGLOSS_VERTEX_CHANNEL});
    #endif

    #ifdef STD_SHEENGLOSS_INVERT
    sheenGlossiness = 1.0 - sheenGlossiness;
    #endif

    sGlossiness = sheenGlossiness + 0.0000001;
}
