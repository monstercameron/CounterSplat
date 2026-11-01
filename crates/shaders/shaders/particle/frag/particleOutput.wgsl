// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particleOutput.js
fn getOutput() -> vec4f {
    if (pcPosition.y < 1.0) {
        return vec4f(outPos, (outAngle + 1000.0) * visMode);
    } else {
        return vec4f(outVel, outLife);
    }
}
