// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/iridescence.js
#ifdef STD_IRIDESCENCE_CONSTANT
    uniform material_iridescence: f32;
#endif

fn getIridescence() {
    var iridescence = 1.0;

    #ifdef STD_IRIDESCENCE_CONSTANT
    iridescence = iridescence * uniform.material_iridescence;
    #endif

    #ifdef STD_IRIDESCENCE_TEXTURE
    iridescence = iridescence * textureSampleBias({STD_IRIDESCENCE_TEXTURE_NAME}, {STD_IRIDESCENCE_TEXTURE_NAME}Sampler, {STD_IRIDESCENCE_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_IRIDESCENCE_TEXTURE_CHANNEL};
    #endif

    dIridescence = iridescence; 
}
