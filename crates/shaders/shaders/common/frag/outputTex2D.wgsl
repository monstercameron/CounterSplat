// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/outputTex2D.js
varying vUv0: vec2f;
var source: texture_2d<f32>;
var sourceSampler: sampler;

@fragment fn fragmentMain(input : FragmentInput) -> FragmentOutput {
    var output: FragmentOutput;
    output.color = textureSample(source, sourceSampler, input.vUv0);
    return output;
}
