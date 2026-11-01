// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/compute-gsplat-projector-write-indirect-args.js
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


fn calcDispatch2D(count: u32, maxDim: u32) -> vec2u {
    if (count <= maxDim) {
        return vec2u(count, 1u);
    }
    let y = (count + maxDim - 1u) / maxDim;
    let x = (count + y - 1u) / y;
    return vec2u(x, y);
}


// Writes up to 3 indirect dispatch slots for a compute-based sorter, using
// sorter-provided metadata. Each slot occupies 3 consecutive u32 entries
// (workgroup count x, y, z) in the dispatch buffer; this helper writes a 1D
// dispatch of ceil(count / granularity[i]) workgroups into slots
// (baseSlot + i) for every active slot i.
//
// Contract: the calling shader must declare a storage binding named
// 'indirectDispatchArgs' of type array<u32> with read_write access. The helper
// references that global directly rather than taking it as a pointer
// parameter, because passing storage pointers across function boundaries
// requires the 'unrestricted_pointer_parameters' WGSL language feature, which
// is not portable (Firefox/naga rejects it).
//
// Parameters:
//   baseSlot  - index of the first slot to write (u32 offset = baseSlot * 3).
//   count     - number of elements the sort will process.
//   slotInfo  - sorter metadata, obtained verbatim from
//               ComputeRadixSort#prepareIndirect() and passed in as a
//               vec4<u32>:
//                 .x = slotCount (0..3; zero skips all sort writes)
//                 .y/.z/.w = per-slot elements-per-workgroup (granularity);
//                            unused trailing entries are 0.
fn writeSortIndirectArgs(
    baseSlot: u32,
    count: u32,
    slotInfo: vec4<u32>
) {
    let n = slotInfo.x;

    if (n >= 1u) {
        let g = slotInfo.y;
        let wc = (count + g - 1u) / g;
        let off = baseSlot * 3u;
        indirectDispatchArgs[off + 0u] = wc;
        indirectDispatchArgs[off + 1u] = 1u;
        indirectDispatchArgs[off + 2u] = 1u;
    }
    if (n >= 2u) {
        let g = slotInfo.z;
        let wc = (count + g - 1u) / g;
        let off = (baseSlot + 1u) * 3u;
        indirectDispatchArgs[off + 0u] = wc;
        indirectDispatchArgs[off + 1u] = 1u;
        indirectDispatchArgs[off + 2u] = 1u;
    }
    if (n >= 3u) {
        let g = slotInfo.w;
        let wc = (count + g - 1u) / g;
        let off = (baseSlot + 2u) * 3u;
        indirectDispatchArgs[off + 0u] = wc;
        indirectDispatchArgs[off + 1u] = 1u;
        indirectDispatchArgs[off + 2u] = 1u;
    }
}


@group(0) @binding(0) var<storage, read> renderCounter: array<u32>;
@group(0) @binding(1) var<storage, read_write> indirectDrawArgs: array<DrawIndexedIndirectArgs>;
@group(0) @binding(2) var<storage, read_write> numSplatsBuf: array<u32>;
@group(0) @binding(3) var<storage, read_write> indirectDispatchArgs: array<u32>;
@group(0) @binding(4) var<storage, read_write> sortElementCountBuf: array<u32>;

struct WriteArgsUniforms {
    drawSlot: u32,
    indexCount: u32,
    sortSlotBase: u32,
    pad0: u32,
    sortIndirectInfo: vec4<u32>
};
@group(0) @binding(5) var<uniform> uniforms: WriteArgsUniforms;

@compute @workgroup_size(1)
fn main() {
    let count = renderCounter[0];
    let instanceCount = (count + {INSTANCE_SIZE}u - 1u) / {INSTANCE_SIZE}u;

    indirectDrawArgs[uniforms.drawSlot] = DrawIndexedIndirectArgs(
        uniforms.indexCount,
        instanceCount,
        0u,
        0,
        0u
    );

    numSplatsBuf[0] = count;
    sortElementCountBuf[0] = count;

    // Sort dispatch args at sortSlotBase (no keygen slot — sort keys come from the
    // projector pass directly).
    writeSortIndirectArgs(uniforms.sortSlotBase, count, uniforms.sortIndirectInfo);
}
