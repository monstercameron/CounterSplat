// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/comp/dispatch-core.js
fn calcDispatch2D(count: u32, maxDim: u32) -> vec2u {
    if (count <= maxDim) {
        return vec2u(count, 1u);
    }
    let y = (count + maxDim - 1u) / maxDim;
    let x = (count + y - 1u) / y;
    return vec2u(x, y);
}
