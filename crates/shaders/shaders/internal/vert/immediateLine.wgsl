// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/internal/vert/immediateLine.js
    attribute vertex_position: vec4f;
    attribute vertex_color: vec4f;
    uniform matrix_model: mat4x4f;
    uniform matrix_viewProjection: mat4x4f;
    varying color: vec4f;
    @vertex
    fn vertexMain(input : VertexInput) -> VertexOutput {
        var output : VertexOutput;
        output.color = input.vertex_color;
        output.position = uniform.matrix_viewProjection * uniform.matrix_model * input.vertex_position;
        return output;
    }
