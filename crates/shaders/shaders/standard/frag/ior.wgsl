// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/ior.js
#ifdef STD_IOR_CONSTANT
    uniform material_refractionIndex: f32;
#endif

fn getIor() {
#ifdef STD_IOR_CONSTANT
    dIor = uniform.material_refractionIndex;
#else
    dIor = 1.0 / 1.5;
#endif
}
