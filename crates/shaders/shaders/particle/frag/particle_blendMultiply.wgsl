// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_blendMultiply.js
    rgb = mix(vec3f(1.0), rgb, a);
    if ((rgb.r + rgb.g + rgb.b) > 2.99) {
        discard;
    }
