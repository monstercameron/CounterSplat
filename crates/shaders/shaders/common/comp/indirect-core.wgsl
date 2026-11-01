// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/comp/indirect-core.js
// Indirect indexed draw call parameters
struct DrawIndexedIndirectArgs {
    indexCount: u32,        // Number of indices to draw
    instanceCount: u32,     // Number of instances to draw
    firstIndex: u32,        // Index of the first index in the index buffer
    baseVertex: i32,        // Offset added to each index before fetching vertex
    firstInstance: u32      // First instance ID
};

// Indirect non-indexed draw call parameters
struct DrawIndirectArgs {
    vertexCount: u32,       // Number of vertices to draw
    instanceCount: u32,     // Number of instances to draw
    firstVertex: u32,       // Index of the first vertex
    firstInstance: u32,     // First instance ID
    _pad: u32               // Padding to match indexed size (unused)
};
