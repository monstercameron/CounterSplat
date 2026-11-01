// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/compute-gsplat-write-indirect-args.js
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


// Prefix sum buffer (flagBuffer after in-place exclusive scan)
@group(0) @binding(0) var<storage, read> prefixSumBuffer: array<u32>;

// Indirect draw buffer (device's shared indirect draw buffer, indexed by slot)
@group(0) @binding(1) var<storage, read_write> indirectDrawArgs: array<DrawIndexedIndirectArgs>;

// numSplats storage buffer for vertex shader to read
@group(0) @binding(2) var<storage, read_write> numSplatsBuf: array<u32>;

// Indirect dispatch buffer (device's shared indirect dispatch buffer)
@group(0) @binding(3) var<storage, read_write> indirectDispatchArgs: array<u32>;

// sortElementCount storage buffer for sort shaders to read (visibleCount)
@group(0) @binding(4) var<storage, read_write> sortElementCountBuf: array<u32>;

// Uniforms
struct WriteArgsUniforms {
    drawSlot: u32,              // slot index into indirectDrawArgs
    indexCount: u32,            // indices per instance (768 = 6 * 128)
    dispatchSlotBase: u32,      // slot index of keygen dispatch; sort slots follow
    totalSplats: u32,           // total splat count, index into prefixSumBuffer for visible count
    sortIndirectInfo: vec4<u32> // sorter-owned [slotCount, g0, g1, g2] from prepareIndirect()
};
@group(0) @binding(5) var<uniform> uniforms: WriteArgsUniforms;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    // Read visible count from prefixSumBuffer[N] where N = totalSplats.
    // After exclusive prefix sum over N+1 elements, prefixSum[N] = total visible count.
    let count = prefixSumBuffer[uniforms.totalSplats];
    let instanceCount = (count + {INSTANCE_SIZE}u - 1u) / {INSTANCE_SIZE}u;

    // Write indexed indirect draw args
    indirectDrawArgs[uniforms.drawSlot] = DrawIndexedIndirectArgs(
        uniforms.indexCount,
        instanceCount,
        0u,     // firstIndex
        0,      // baseVertex
        0u      // firstInstance
    );

    // Write numSplats for vertex shader
    numSplatsBuf[0] = count;

    // --- Indirect dispatch args ---
    // Layout: slot[base + 0] = key gen, slot[base + 1..base + 1 + sortSlotCount] = sort.
    let keygenSlot = uniforms.dispatchSlotBase;
    let keygenOffset = keygenSlot * 3u;

    let keygenWorkgroupCount = (count + {KEYGEN_THREADS_PER_WORKGROUP}u - 1u) / {KEYGEN_THREADS_PER_WORKGROUP}u;
    let keygenDim = calcDispatch2D(keygenWorkgroupCount, {MAX_WORKGROUPS_PER_DIM}u);
    indirectDispatchArgs[keygenOffset + 0u] = keygenDim.x;
    indirectDispatchArgs[keygenOffset + 1u] = keygenDim.y;
    indirectDispatchArgs[keygenOffset + 2u] = 1u;

    // Sort dispatch slots — delegated to the sorter-agnostic helper so this
    // shader doesn't need to know how many slots or what granularity the
    // active radix sort backend uses.
    writeSortIndirectArgs(keygenSlot + 1u, count, uniforms.sortIndirectInfo);

    // Write sortElementCount for sort shaders (= visibleCount)
    sortElementCountBuf[0] = count;
}
