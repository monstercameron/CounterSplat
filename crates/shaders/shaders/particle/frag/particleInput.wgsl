// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particleInput.js
fn readInput(uv: f32) {
    let textureSize = textureDimensions(particleTexIN, 0);
    let texel0: vec2i = vec2i(vec2f(uv, 0.25) * vec2f(textureSize));
    let texel1: vec2i = vec2i(vec2f(uv, 0.75) * vec2f(textureSize));
    let tex: vec4f = textureLoad(particleTexIN, texel0, 0);
    let tex2: vec4f = textureLoad(particleTexIN, texel1, 0);

    inPos = tex.xyz;
    inVel = tex2.xyz;
    inAngle = abs(tex.w) - 1000.0;
    inShow = tex.w >= 0.0;
    inLife = tex2.w;
}
