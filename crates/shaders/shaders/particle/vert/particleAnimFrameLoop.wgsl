// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particleAnimFrameLoop.js
    let animFrame: f32 = floor((output.texCoordsAlphaLife.w * uniform.animTexParams.y + uniform.animTexParams.x) % (uniform.animTexParams.z + 1.0));    
