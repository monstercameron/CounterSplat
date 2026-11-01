// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/thickness.js
#ifdef STD_THICKNESS_CONSTANT
uniform material_thickness: f32;
#endif

fn getThickness() {
    dThickness = 1.0;

    #ifdef STD_THICKNESS_CONSTANT
    dThickness = dThickness * uniform.material_thickness;
    #endif

    #ifdef STD_THICKNESS_TEXTURE
    dThickness = dThickness * textureSampleBias({STD_THICKNESS_TEXTURE_NAME}, {STD_THICKNESS_TEXTURE_NAME}Sampler, {STD_THICKNESS_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_THICKNESS_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_THICKNESS_VERTEX
    dThickness = dThickness * saturate(vVertexColor.{STD_THICKNESS_VERTEX_CHANNEL});
    #endif
}
