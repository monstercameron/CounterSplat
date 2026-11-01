// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/sheen.js
uniform material_sheen: vec3f;

fn getSheen() {
    var sheenColor = uniform.material_sheen;

    #ifdef STD_SHEEN_TEXTURE
    sheenColor = sheenColor * {STD_SHEEN_TEXTURE_DECODE}(textureSampleBias({STD_SHEEN_TEXTURE_NAME}, {STD_SHEEN_TEXTURE_NAME}Sampler, {STD_SHEEN_TEXTURE_UV}, {STD_TEXTURE_BIAS})).{STD_SHEEN_TEXTURE_CHANNEL};
    #endif

    #ifdef STD_SHEEN_VERTEX
    sheenColor = sheenColor * saturate(vVertexColor.{STD_SHEEN_VERTEX_CHANNEL});
    #endif

    sSpecularity = sheenColor;
}
