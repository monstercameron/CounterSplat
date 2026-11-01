// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/standard/frag/alphaTest.js
uniform alpha_ref: f32;

fn alphaTest(a: f32) {
    if (a < uniform.alpha_ref) {
        discard;
    }
}
