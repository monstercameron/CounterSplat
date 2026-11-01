// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/internal/frag/cookie-blit-cube.js
    varying uv0: vec2f;
    uniform invViewProj: mat4x4<f32>;
    var blitTexture: texture_cube<f32>;
    var blitTextureSampler : sampler;

    @fragment
    fn fragmentMain(input : FragmentInput) -> FragmentOutput {
        var output: FragmentOutput;
        var projPos = vec4f(input.uv0 * 2.0 - 1.0, 0.5, 1.0);
        var worldPos = uniform.invViewProj * projPos;
        output.color = textureSample(blitTexture, blitTextureSampler, worldPos.xyz);
        return output;
    }
