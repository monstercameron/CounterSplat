// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/emissive.js
#ifndef MESH_COLOR
    uniform material_emissive: vec3f;
#endif
uniform material_emissiveIntensity: f32;

fn getEmission() {
    #ifdef MESH_COLOR
        dEmission = uniform.mesh_color.rgb * uniform.material_emissiveIntensity;
    #else
        dEmission = uniform.material_emissive * uniform.material_emissiveIntensity;
    #endif

    #ifdef STD_EMISSIVE_TEXTURE
    dEmission *= {STD_EMISSIVE_TEXTURE_DECODE}(textureSampleBias({STD_EMISSIVE_TEXTURE_NAME}, {STD_EMISSIVE_TEXTURE_NAME}Sampler, {STD_EMISSIVE_TEXTURE_UV}, {STD_TEXTURE_BIAS})).{STD_EMISSIVE_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_EMISSIVE_VERTEX
    dEmission = dEmission * saturate(vVertexColor.{STD_EMISSIVE_VERTEX_CHANNEL});
    #endif
}
