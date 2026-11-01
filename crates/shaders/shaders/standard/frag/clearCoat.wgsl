// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/clearCoat.js
uniform material_clearCoat: f32;

fn getClearCoat() {
    ccSpecularity = uniform.material_clearCoat;

    #ifdef STD_CLEARCOAT_TEXTURE
    ccSpecularity = ccSpecularity * textureSampleBias({STD_CLEARCOAT_TEXTURE_NAME}, {STD_CLEARCOAT_TEXTURE_NAME}Sampler, {STD_CLEARCOAT_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_CLEARCOAT_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_CLEARCOAT_VERTEX
    ccSpecularity = ccSpecularity * saturate(vVertexColor.{STD_CLEARCOAT_VERTEX_CHANNEL});
    #endif
}
