// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/vert/meshInstanceStorage.js
var<storage, read> meshInstanceStorage: array<vec4f>;

fn getStoredModelMatrix() -> mat4x4f {
    let base = pcInstanceIndex * 7u;
    return mat4x4f(
        meshInstanceStorage[base],
        meshInstanceStorage[base + 1u],
        meshInstanceStorage[base + 2u],
        meshInstanceStorage[base + 3u]
    );
}

fn getStoredNormalMatrix() -> mat3x3f {
    let base = pcInstanceIndex * 7u;
    return mat3x3f(
        meshInstanceStorage[base + 4u].xyz,
        meshInstanceStorage[base + 5u].xyz,
        meshInstanceStorage[base + 6u].xyz
    );
}
