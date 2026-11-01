// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/base.js
uniform view_position: vec3f;

uniform light_globalAmbient: vec3f;

fn square(x: f32) -> f32 {
    return x*x;
}
