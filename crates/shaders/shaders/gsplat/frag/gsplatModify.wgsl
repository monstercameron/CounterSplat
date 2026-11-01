// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/frag/gsplatModify.js
// Modify the final splat fragment color in the forward pass.
// Parameters:
//   gaussianUV - position of the fragment within the gaussian footprint: (0,0) at the splat
//                center, length 1 at the edge where the splat is clipped
//   color      - rgb: splat color, a: fragment alpha after gaussian falloff, before
//                premultiplication
fn modifySplatColor(gaussianUV: vec2f, color: ptr<function, vec4f>) {
    // Example: *color = vec4f((*color).rgb * 0.5, (*color).a); // darken all splats
}
