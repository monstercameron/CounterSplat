// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/vert/gsplatSplat.js
// Splat identification for texture sampling
struct Splat {
    index: u32,     // linear index into splat data
    uv: vec2i       // texture coordinate for sampling
}

// Global splat instance used by format read functions and load functions
var<private> splat: Splat;

// Initialize splat from linear index
fn setSplat(idx: u32) {
    splat.index = idx;
    splat.uv = vec2i(i32(idx % uniform.splatTextureSize), i32(idx / uniform.splatTextureSize));
}

