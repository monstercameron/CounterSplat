// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/compute-gsplat-shadow-indirect-args.js
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


// Visible splat count (element 0), produced by the shadow cull's atomic counter.
@group(0) @binding(0) var<storage, read> countBuffer: array<u32>;

// Device's shared indirect draw buffer, indexed by slot.
@group(0) @binding(1) var<storage, read_write> indirectDrawArgs: array<DrawIndexedIndirectArgs>;

struct ShadowArgsUniforms {
    drawSlot: u32,      // slot index into indirectDrawArgs
    indexCount: u32,    // indices per instance (768 = 6 * 128)
    pad0: u32,
    pad1: u32
};
@group(0) @binding(2) var<uniform> uniforms: ShadowArgsUniforms;

@compute @workgroup_size(1)
fn main() {
    let count = countBuffer[0];
    let instanceCount = (count + {INSTANCE_SIZE}u - 1u) / {INSTANCE_SIZE}u;

    indirectDrawArgs[uniforms.drawSlot] = DrawIndexedIndirectArgs(
        uniforms.indexCount,
        instanceCount,
        0u,     // firstIndex
        0,      // baseVertex
        0u      // firstInstance
    );
}
