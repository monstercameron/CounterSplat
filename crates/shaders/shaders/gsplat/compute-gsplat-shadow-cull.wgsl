// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/compute-gsplat-shadow-cull.js
// Conservative gaussian extent (in std-devs) for the per-splat frustum radius, so a splat whose
// tail still reaches into the light frustum is not culled at the boundary.
const SPLAT_FRUSTUM_SIGMA: f32 = 3.0;

struct CullUniforms {
    frustumPlanes: array<vec4f, 6>,
    numIntervals: u32,
    splatTextureSize: u32,
    alphaClip: f32,
    worldSizeThreshold: f32
};
@group(0) @binding(0) var<uniform> uniforms: CullUniforms;

// Pass-1 outputs (read): the shared candidate list + the candidate count at [numIntervals].
@group(0) @binding(1) var<storage, read> compactedSplatIds: array<u32>;
@group(0) @binding(2) var<storage, read> candidateCountBuffer: array<u32>;

// Per-light outputs: dense visible work-buffer indices (unordered) + a single atomic visible count.
@group(0) @binding(3) var<storage, read_write> outputIndices: array<u32>;
@group(0) @binding(4) var<storage, read_write> globalCount: array<atomic<u32>>;

// Work-buffer format texture bindings (binding 5+) and the splat read/modify helpers. setSplat
// (gsplatComputeSplatCS) reads uniforms.splatTextureSize, so the uniform struct is declared above.
#include "dispatchThreadsCS"
#include "gsplatComputeSplatCS"
#include "gsplatFormatDeclCS"
#include "gsplatFormatReadCS"
#include "gsplatHelpersVS"
#include "gsplatModifyVS"

// Fine per-splat cull: read + apply the render-stage modifier + opacity/size/frustum tests. Returns
// true when the splat should be drawn into the shadow map.
fn fineCull(splatId: u32) -> bool {
    setSplat(splatId);

    // world-space center + render-stage position modifier (matches the quad VS / forward projector)
    let originalCenter = getCenter();
    var center = originalCenter;
    modifySplatCenter(&center);

    // opacity cull (matches the forward alpha cull + the shadow PS alphaClip)
    let opacity = getOpacity();
    if (opacity <= uniforms.alphaClip) {
        return false;
    }

    // render-stage rotation/scale modifier; getRotation() is (w,x,y,z), the hook contract is (x,y,z,w)
    var rotation: vec4f = getRotation().yzwx;
    var scale: vec3f = getScale();
    modifySplatRotationScale(originalCenter, center, &rotation, &scale);

    // size cull: the orthographic shadow projection is linear, so a splat whose max world-space
    // extent maps below the precomputed per-light threshold is sub-pixel in the shadow map. The max
    // scale axis is an orientation-independent upper bound on the projected radius (conservative —
    // never culls a splat the forward path would keep). A threshold of 0 disables the test.
    let maxScale = max(scale.x, max(scale.y, scale.z));
    if (maxScale < uniforms.worldSizeThreshold) {
        return false;
    }

    // per-splat frustum cull: the modified center (± a conservative gaussian radius) against the
    // light's 6 frustum planes. Trims splats in boundary nodes; the ortho near/far planes also cull
    // by depth.
    let splatRadius = maxScale * SPLAT_FRUSTUM_SIGMA;
    for (var p = 0; p < 6; p++) {
        let plane = uniforms.frustumPlanes[p];
        if (dot(plane.xyz, center) + plane.w <= -splatRadius) {
            return false;
        }
    }

    return true;
}

// One global atomicAdd per workgroup; intra-workgroup slots via a workgroup atomic (no subgroups).
var<workgroup> wgCount: atomic<u32>;
var<workgroup> wgBase: u32;

@compute @workgroup_size({WORKGROUP_SIZE})
fn main(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(local_invocation_index) localIdx: u32
) {
    if (localIdx == 0u) {
        atomicStore(&wgCount, 0u);
    }
    workgroupBarrier();

    // the flat thread index, also when the dispatch is spread over rows of workgroups
    let threadIdx = dispatchThreadIndex(gid);
    let candidateCount = candidateCountBuffer[uniforms.numIntervals];

    var valid = false;
    var splatId = 0u;
    if (threadIdx < candidateCount) {
        splatId = compactedSplatIds[threadIdx];
        valid = fineCull(splatId);
    }

    var localSlot = 0u;
    if (valid) {
        localSlot = atomicAdd(&wgCount, 1u);
    }
    workgroupBarrier();

    // workgroup leader reserves a contiguous global block for this workgroup's survivors
    if (localIdx == 0u) {
        wgBase = atomicAdd(&globalCount[0], atomicLoad(&wgCount));
    }
    workgroupBarrier();

    if (valid) {
        outputIndices[wgBase + localSlot] = splatId;
    }
}
