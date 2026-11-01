// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/internal/frag/cookie-blit-2d.js
    varying uv0: vec2f;

    var blitTexture: texture_2d<f32>;
    var blitTextureSampler : sampler;

    @fragment
    fn fragmentMain(input : FragmentInput) -> FragmentOutput {
        var output: FragmentOutput;
        output.color = textureSample(blitTexture, blitTextureSampler, input.uv0);
        return output;
    }
