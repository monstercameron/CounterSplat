// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/opacity.js
#ifndef MESH_COLOR
    uniform material_opacity: f32;
#endif
uniform material_alphaDitherScale: f32;

fn getOpacity() {
    #ifdef MESH_COLOR
        dAlpha = uniform.mesh_color.a;
    #else
        dAlpha = uniform.material_opacity;
    #endif

    #ifdef STD_OPACITY_TEXTURE
    dAlpha = dAlpha * textureSampleBias({STD_OPACITY_TEXTURE_NAME}, {STD_OPACITY_TEXTURE_NAME}Sampler, {STD_OPACITY_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_OPACITY_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_OPACITY_VERTEX
    dAlpha = dAlpha * clamp(vVertexColor.{STD_OPACITY_VERTEX_CHANNEL}, 0.0, 1.0);
    #endif
}
