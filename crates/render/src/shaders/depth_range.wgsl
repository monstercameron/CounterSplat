// Limits the splat depth prepass to the splats that can affect a depth test: those no farther than `far_depth`.
//
// The prepass writes each splat's centre depth over its solid core, and only the impact decals and smoke test against
// it (`LessEqual`, no depth writes, every fragment no farther than `far_depth`). A splat whose centre is farther than
// that writes a depth that every such fragment passes anyway, so leaving it out cannot change any pixel.
//
// The sorted order runs farthest first, so those splats are a prefix of it. Its end is found by a binary search that
// recomputes each probed splat's depth level exactly as `depth_keys.wgsl` builds its key; the levels never increase
// along the order. The cut is taken a little past `far_depth` (0.2%, about seven key steps), so a level that differs by
// a step between the two shaders (different instruction scheduling) still lands on the drawn side.
//
// One thread; about 23 probes for 6M splats.

struct RangeUniforms {
    view_model: mat4x4f,
    // The farthest view-space depth any depth-tested fragment reaches (scene units).
    far_depth: f32,
    // 0: draw every splat in view (no impacts, or the caller did not limit the range).
    enabled: u32,
    index_count: u32,
    instance_size: u32,
};

@group(0) @binding(0) var<uniform> params: RangeUniforms;
@group(0) @binding(1) var<storage, read> centers: array<f32>;
// The sorted order (farthest first) and how many of its entries are in view.
@group(0) @binding(2) var<storage, read> order: array<u32>;
@group(0) @binding(3) var<storage, read> visible: array<u32>;
// The first entry of the order the prepass draws; the prepass vertex stage adds it to its splat position.
@group(0) @binding(4) var<storage, read_write> start: array<u32>;
// drawIndexedIndirect: indexCount, instanceCount, firstIndex, baseVertex, firstInstance.
@group(0) @binding(5) var<storage, read_write> args: array<u32, 5>;

// Must match depth_keys.wgsl (checked by a test in sort.rs).
const KEY_LOG2_NEAR: f32 = -9.965784;
const KEY_STEPS_PER_LOG2: f32 = 2466.0;

fn depth_level(depth: f32) -> u32 {
    var level = 0.0;
    if (depth > 0.0) {
        level = clamp((log2(depth) - KEY_LOG2_NEAR) * KEY_STEPS_PER_LOG2, 0.0, 65535.0);
    }
    return u32(level);
}

fn splat_level(slot: u32) -> u32 {
    let c = vec3f(centers[slot * 3u], centers[slot * 3u + 1u], centers[slot * 3u + 2u]);
    let v = (params.view_model * vec4f(c, 1.0)).xyz;
    return depth_level(-v.z);
}

@compute @workgroup_size(1)
fn main() {
    let n = min(visible[0], arrayLength(&order));
    var first = 0u;
    if (params.enabled != 0u) {
        let cut = depth_level(params.far_depth * 1.002);
        // The first entry at or nearer than the cut: every entry before it is farther.
        var lo = 0u;
        var hi = n;
        while (lo < hi) {
            let mid = lo + (hi - lo) / 2u;
            if (splat_level(order[mid]) > cut) {
                lo = mid + 1u;
            } else {
                hi = mid;
            }
        }
        first = lo;
    }
    start[0] = first;
    args[0] = params.index_count;
    args[1] = (n - first + params.instance_size - 1u) / params.instance_size;
    args[2] = 0u;
    args[3] = 0u;
    args[4] = 0u;
}
