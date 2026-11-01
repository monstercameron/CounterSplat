// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/iridescenceThickness.js
uniform material_iridescenceThicknessMax: f32;

#ifdef STD_IRIDESCENCETHICKNESS_TEXTURE
    uniform material_iridescenceThicknessMin: f32;
#endif

fn getIridescenceThickness() {

    #ifdef STD_IRIDESCENCETHICKNESS_TEXTURE
        var blend: f32 = textureSampleBias({STD_IRIDESCENCETHICKNESS_TEXTURE_NAME}, {STD_IRIDESCENCETHICKNESS_TEXTURE_NAME}Sampler, {STD_IRIDESCENCETHICKNESS_TEXTURE_UV}, {STD_TEXTURE_BIAS}).{STD_IRIDESCENCETHICKNESS_TEXTURE_CHANNEL};
        var iridescenceThickness: f32 = mix(uniform.material_iridescenceThicknessMin, uniform.material_iridescenceThicknessMax, blend);
    #else
        var iridescenceThickness: f32 = uniform.material_iridescenceThicknessMax;
    #endif

    dIridescenceThickness = iridescenceThickness; 
}
