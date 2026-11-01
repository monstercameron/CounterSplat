// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/aoDiffuseOcc.js
fn occludeDiffuse(ao: f32) {
    dDiffuseLight = dDiffuseLight * ao;
}
