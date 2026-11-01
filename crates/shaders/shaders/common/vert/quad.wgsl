// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/vert/quad.js
    attribute aPosition: vec2f;
    varying uv0: vec2f;
    @vertex fn vertexMain(input: VertexInput) -> VertexOutput {
        var output: VertexOutput;
        output.position = vec4f(input.aPosition, 0.0, 1.0);
        output.uv0 = getImageEffectUV((input.aPosition + 1.0) * 0.5);
        return output;
    }
