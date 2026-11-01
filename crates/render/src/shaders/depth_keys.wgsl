// Order-preserving depth keys for the radix sort. Ascending keys follow ascending view-space z, so the
// farthest splat (most negative z, since the camera looks down -z) sorts first: the back-to-front order
// that premultiplied "over" blending needs.
//
// Splats whose centre is behind the camera, or well outside the view, and empty slots get the largest key, so they
// sort after every visible splat; `visible` counts the rest, and `drawArgs` turns that count into the instanced draw's arguments, so
// the draws only shade the visible prefix of the order.
//
// An optional target cutout culls splats between the camera and a world-space target. Its edge dissolves over
// a band (a per-splat hash decides); geometry below the supplied world-space floor height is retained.
//
// Crossfades (a streamed level changing a leaf's level of detail): `fades` lists slot ranges, sorted by start, each
// with the share of its opacity to draw. Every splat's share goes into `fade` (1 outside the ranges), which the splat
// shaders multiply into its opacity, so a new range fades in smoothly over the old one, which then fades out. A
// splat at share 0 is left out of the draw.

struct DepthUniforms {
    view_model: mat4x4f,
    clip: mat4x4f,
    // Splat space to world space, for the cutout's floor test.
    cut_model: mat4x4f,
    // xyz: the target's centre in view space; w: how far a splat reaches past its centre (metres).
    cut_target: vec4f,
    // x: hole radius (metres at the target), y: world height kept below, z: depth margin in front of the target,
    // w: the soft edge's outer radius as a multiple of the hole's (0 when the cutout is off).
    cut_params: vec4f,
    count: u32,
    fade_count: u32,
};

@group(0) @binding(0) var<uniform> params: DepthUniforms;
@group(0) @binding(1) var<storage, read> centers: array<f32>;
@group(0) @binding(2) var<storage, read_write> keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> visible: atomic<u32>;
// x: first slot, y: one past the last, z: the share drawn (f32 bits).
@group(0) @binding(4) var<storage, read> fades: array<vec4u>;
// Per splat: the share of its opacity drawn.
@group(0) @binding(5) var<storage, read_write> fade: array<f32>;

// A centre this far outside the view (in units of the half-screen) can still reach into it with a large splat.
const CULL_MARGIN: f32 = 1.5;


fn hash01(i: u32) -> f32 {
    var x = i * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return f32(x) / 4294967295.0;
}

// True when the splat at view-space `p` (model-space `c`, index `i`) hides the target and should be cut.
fn in_cutout(p: vec3f, c: vec3f, i: u32) -> bool {
    let cut = params.cut_params;
    let target_pos = params.cut_target.xyz;
    if (cut.w <= 1.0 || target_pos.z >= -0.01) {
        return false;
    }
    // Only what is between the camera and the target (closer than the target, less a margin) and in front of the camera.
    if (p.z <= target_pos.z + cut.z || p.z >= 0.0) {
        return false;
    }
    // Geometry below the configured floor stays.
    if ((params.cut_model * vec4f(c, 1.0)).y < cut.y) {
        return false;
    }
    // Angular distance from the target's centre on screen, against the target's angular radius.
    let d = length(p.xy / -p.z - target_pos.xy / -target_pos.z);
    // Near the camera a splat covers a large angle, so a splat whose centre is just outside the hole would still
    // cover it; the hole is widened by the splat's reach seen from its depth.
    let r = cut.x / -target_pos.z + params.cut_target.w / -p.z;
    if (d <= r) {
        return true;
    }
    let t = (d - r) / (r * (cut.w - 1.0));
    return t < 1.0 && hash01(i) > t;
}

// The share of splat `i`'s opacity drawn now: its fading range's share, or 1.
fn fade_share(i: u32) -> f32 {
    // The last range starting at or before i.
    var lo = 0u;
    var hi = params.fade_count;
    while (lo < hi) {
        let mid = (lo + hi) / 2u;
        if (fades[mid].x <= i) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    if (lo == 0u) {
        return 1.0;
    }
    let f = fades[lo - 1u];
    return select(1.0, bitcast<f32>(f.z), i < f.y);
}

// Keep the global count to one atomic per workgroup rather than one per visible Gaussian.
var<workgroup> local_visible: atomic<u32>;

fn write_key(i: u32) -> bool {
    if (i >= params.count) {
        return false;
    }
    // An empty slot (released by a streamed level) has a NaN centre. Comparisons with NaN are all false, so the
    // tests below would let it through and its stale splat would still be drawn; the exponent bits catch it.
    if ((bitcast<u32>(centers[i * 3u]) & 0x7f800000u) == 0x7f800000u) {
        fade[i] = 0.0;
        keys[i] = 0xffffffffu;
        return false;
    }
    let share = fade_share(i);
    fade[i] = share;
    let c = vec3f(centers[i * 3u], centers[i * 3u + 1u], centers[i * 3u + 2u]);
    let p = params.clip * vec4f(c, 1.0);
    if (p.w <= 0.0 || abs(p.x) > p.w * CULL_MARGIN || abs(p.y) > p.w * CULL_MARGIN || share <= 0.0) {
        fade[i] = 0.0; // Direct draws still visit active slots; culled entries must remain transparent.
        keys[i] = 0xffffffffu;
        return false;
    }
    let v = (params.view_model * vec4f(c, 1.0)).xyz;
    if (in_cutout(v, c, i)) {
        fade[i] = 0.0;
        keys[i] = 0xffffffffu;
        return false;
    }
    let bits = bitcast<u32>(v.z);
    // Flip the sign bit for positives, and every bit for negatives, so unsigned order equals float order.
    keys[i] = select(bits | 0x80000000u, ~bits, (bits & 0x80000000u) != 0u);
    return true;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) lane: u32) {
    if (lane == 0u) { atomicStore(&local_visible, 0u); }
    workgroupBarrier();
    let shown = write_key(id.x);
    // All lanes issue the local atomic, including padding in the last group (adding zero).
    // This also avoids the partial-wave atomic issue seen on Adreno.
    atomicAdd(&local_visible, select(0u, 1u, shown));
    workgroupBarrier();
    if (lane == 0u) { atomicAdd(&visible, atomicLoad(&local_visible)); }
}

// Direct draws already cover the active prefix and reject culled slots in the
// vertex stage. They need the same keys and fades but no visibility count.
@compute @workgroup_size(256)
fn direct(@builtin(global_invocation_id) id: vec3u) {
    let ignored = write_key(id.x);
}
