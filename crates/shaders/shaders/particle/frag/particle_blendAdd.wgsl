// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle_blendAdd.js
    dBlendModeFogFactor = 0.0;
    rgb = rgb * saturate(gammaCorrectInput(max(a, 0.0)));
    if ((rgb.r + rgb.g + rgb.b) < 0.000001) {
        discard;
    }    
