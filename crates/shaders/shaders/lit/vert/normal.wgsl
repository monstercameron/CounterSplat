// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/vert/normal.js
var<private> dNormalMatrix: mat3x3f;

fn getNormal() -> vec3f {
    dNormalMatrix = getNormalMatrix(dModelMatrix);
    let localNormal: vec3f = getLocalNormal(vertex_normal);
    return normalize(dNormalMatrix * localNormal);
}