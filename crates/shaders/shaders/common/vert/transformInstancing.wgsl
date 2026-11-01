// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/vert/transformInstancing.js
attribute instance_line1: vec4f;
attribute instance_line2: vec4f;
attribute instance_line3: vec4f;
attribute instance_line4: vec4f;

fn getModelMatrix() -> mat4x4f {
    return uniform.matrix_model * mat4x4f(instance_line1, instance_line2, instance_line3, instance_line4);
}
