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
    // The vertex stage's footprint tests (see `footprint_culled`). x, y: its focal lengths (viewport size times the
    // projection's x and y scale); z, w: the viewport in pixels.
    footprint_focal: vec4f,
    // x: minPixelSize; y: a bound on the squared stretch of view_model's 3x3 part; z: depth-of-field focus distance;
    // w: depth-of-field blur scale (pixels, 0 when off).
    footprint_params: vec4f,
    // x: the most variance motion blur can add (0 when off); y: 1 when the cull is on; z: the kernel's size clamp
    // (vmin); w: the splat textures' width (u32 bits).
    footprint_extra: vec4f,
    // How this pass writes `fade` (see `put_fade`). Not part of the sort's cache key, nor are the two below.
    fade_mode: u32,
    // The first 256-slot group `group_bounds` recomputes.
    bounds_first: u32,
    // 1 when `main` may skip a group whose box of centres is wholly outside the view (compacting sorts only: the
    // skipped slots' keys are left stale, and `compact` skips groups with nothing in view).
    group_cull: u32,
};

@group(0) @binding(0) var<uniform> params: DepthUniforms;
@group(0) @binding(1) var<storage, read> centers: array<f32>;
@group(0) @binding(2) var<storage, read_write> keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> visible: atomic<u32>;
// x: first slot, y: one past the last, z: the share drawn (f32 bits).
@group(0) @binding(4) var<storage, read> fades: array<vec4u>;
// Per splat: the share of its opacity drawn.
@group(0) @binding(5) var<storage, read_write> fade: array<f32>;
// Compaction (indirect sorts only): the splats in view per 256-slot group, then each group's first output position.
@group(0) @binding(6) var<storage, read_write> group_counts: array<u32>;
// The radix sort's first-pass inputs: only the splats in view, in slot order, with their slot as the value.
@group(0) @binding(7) var<storage, read_write> compact_keys: array<u32>;
@group(0) @binding(8) var<storage, read_write> compact_values: array<u32>;
// The splats' scale texture (SOG v2 codebook indices) and the 256-entry codebook (log scale in .r), as the vertex
// stage reads them. 1x1 stand-ins when the cull is off.
@group(0) @binding(9) var splat_scales: texture_2d<f32>;
@group(0) @binding(10) var splat_codebook: texture_2d<f32>;

// Per 256-slot group, the box around its splats' centres (min > max when every slot is empty), refreshed by
// `group_bounds` for the groups whose centres changed. Stored as f32 bits after the per-group counts in
// `group_counts` (six words a group), which keeps the pass within WebGPU's default of eight storage buffers.
fn bounds_base() -> u32 {
    return (arrayLength(&centers) / 3u + 255u) / 256u;
}

fn store_bounds(g: u32, lo: vec3f, hi: vec3f) {
    let at = bounds_base() + g * 6u;
    group_counts[at] = bitcast<u32>(lo.x);
    group_counts[at + 1u] = bitcast<u32>(lo.y);
    group_counts[at + 2u] = bitcast<u32>(lo.z);
    group_counts[at + 3u] = bitcast<u32>(hi.x);
    group_counts[at + 4u] = bitcast<u32>(hi.y);
    group_counts[at + 5u] = bitcast<u32>(hi.z);
}

fn load_bound(g: u32, which: u32) -> vec3f {
    let at = bounds_base() + g * 6u + which * 3u;
    return bitcast<vec3f>(vec3u(group_counts[at], group_counts[at + 1u], group_counts[at + 2u]));
}

// A centre this far outside the view (in units of the half-screen) can still reach into it with a large splat.
const CULL_MARGIN: f32 = 1.5;
// The depth key's range: 1 mm to 100 km in scene units, 65,536 logarithmic steps (log2(1e8) = 26.575).
const KEY_LOG2_NEAR: f32 = -9.965784;
const KEY_STEPS_PER_LOG2: f32 = 2466.0;


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

// True when the vertex stage is sure to discard splat `i` (clip-space centre `p`, view-space centre `v`): its
// footprint is under minPixelSize, or off screen even at its full extent. Those splats draw nothing, so leaving them
// out of the sort and the draw changes no pixel; they would otherwise each cost four vertex invocations.
//
// The vertex stage projects the covariance R S S R^T through W (view_model's 3x3) and the perspective Jacobian J.
// Its largest eigenvalue is at most s_max^2 |W|^2 |J|^2, with |J|^2 bounded by J's Frobenius norm; the stage then adds
// 0.3 to the diagonal, plus depth-of-field and motion-blur variance when those are on. The bound only ever grows the
// footprint (with a margin for the vertex stage's half-precision scale), so a cut splat is one the stage discards.
fn footprint_culled(i: u32, p: vec4f, v: vec3f) -> bool {
    let extra = params.footprint_extra;
    if (extra.y == 0.0) {
        return false;
    }
    let z = -v.z;
    if (!(z > 1e-6)) {
        return false;
    }
    let width = bitcast<u32>(extra.w);
    let s = textureLoad(splat_scales, vec2i(i32(i % width), i32(i / width)), 0).xyz;
    let b = vec3i(s * 255.0 + 0.5);
    let log_max = max(textureLoad(splat_codebook, vec2i(b.x, 0), 0).r,
        max(textureLoad(splat_codebook, vec2i(b.y, 0), 0).r, textureLoad(splat_codebook, vec2i(b.z, 0), 0).r));
    let s_max = exp(log_max);
    let focal = params.footprint_focal;
    let fp = params.footprint_params;
    let z2 = z * z;
    let jacobian2 = (focal.x * focal.x * (z2 + v.x * v.x) + focal.y * focal.y * (z2 + v.y * v.y)) / (z2 * z2);
    var lambda = s_max * s_max * fp.y * jacobian2 * 1.02 + 0.3;
    if (fp.w > 0.0) {
        let diameter = min(24.0, fp.w * abs(1.0 - fp.z / max(z, 0.001)));
        lambda += diameter * diameter / 16.0 * 1.02;
    }
    lambda += extra.x;
    // The vertex stage's kernel extent in its units (`l1` in gsplatCorner), never larger than this bound.
    let l = 2.0 * min(sqrt(2.0 * lambda), extra.z);
    if (l < fp.x * 0.99) {
        return true;
    }
    // Its frustum test: |proj.xy| - l * proj.w / viewport > proj.w culls.
    let reach = p.w * (vec2f(1.0) + vec2f(l) / focal.zw) * 1.001 + vec2f(1e-6);
    return any(abs(p.xy) > reach);
}

// Keep the global count to one atomic per workgroup rather than one per visible Gaussian.
var<workgroup> local_visible: atomic<u32>;

// Writes splat `i`'s share into `fade` as `fade_mode` says: 0 (direct draws, which visit every active slot) the share,
// or 0 for a culled slot so it stays transparent; 1 (compacting sorts, which never draw a culled slot) the share
// whether culled or not, so the buffer is right for any later view; 2 nothing, as the shares have not changed since a
// mode-1 pass wrote them all. Mode 2 saves a write of 4 bytes a slot on every sort.
fn put_fade(i: u32, share: f32, culled: bool) {
    if (params.fade_mode == 0u) {
        fade[i] = select(share, 0.0, culled);
    } else if (params.fade_mode == 1u) {
        fade[i] = share;
    }
}

fn cull(i: u32, share: f32) -> bool {
    put_fade(i, share, true);
    keys[i] = 0xffffffffu;
    return false;
}

fn write_key(i: u32) -> bool {
    if (i >= params.count) {
        return false;
    }
    // An empty slot (released by a streamed level) has a NaN centre. Comparisons with NaN are all false, so the
    // tests below would let it through and its stale splat would still be drawn; the exponent bits catch it.
    if ((bitcast<u32>(centers[i * 3u]) & 0x7f800000u) == 0x7f800000u) {
        // Its share, not 0: a share depends only on the slot and the fade ranges, and with fade writes skipped (mode
        // 2) a splat uploaded into this slot later must find its share already in place.
        return cull(i, fade_share(i));
    }
    let share = fade_share(i);
    let c = vec3f(centers[i * 3u], centers[i * 3u + 1u], centers[i * 3u + 2u]);
    let p = params.clip * vec4f(c, 1.0);
    if (p.w <= 0.0 || abs(p.x) > p.w * CULL_MARGIN || abs(p.y) > p.w * CULL_MARGIN || share <= 0.0) {
        return cull(i, share);
    }
    let v = (params.view_model * vec4f(c, 1.0)).xyz;
    if (in_cutout(v, c, i) || footprint_culled(i, p, v)) {
        return cull(i, share);
    }
    // A 16-bit logarithmic depth key in the top half of the word (the radix passes sort bits 16..31). Over
    // KEY_NEAR..KEY_FAR (scene units) a step is 0.028% of the depth: finer than the 11 mantissa bits the float key
    // kept before (0.05%), and one radix pass fewer. Ascending keys still run farthest first. Splats behind the
    // camera (only possible without a clip test) all share the last key and keep their slot order.
    let depth = -v.z;
    var level = 0.0;
    if (depth > 0.0) {
        level = clamp((log2(depth) - KEY_LOG2_NEAR) * KEY_STEPS_PER_LOG2, 0.0, 65535.0);
    }
    let key = (65535u - u32(level)) << 16u;
    // The all-ones key marks a culled slot; a (NaN-depth) splat that happened to produce it is culled too, so the
    // visible count and the compaction below always agree.
    if (key == 0xffffffffu) {
        return cull(i, share);
    }
    put_fade(i, share, false);
    keys[i] = key;
    return true;
}

var<workgroup> bound_lo: array<vec3f, 256>;
var<workgroup> bound_hi: array<vec3f, 256>;

// Recomputes the box of group `bounds_first + workgroup`. Empty slots (non-finite x, as `write_key` tests) are left
// out; a slot with a finite x but another non-finite coordinate makes the box unbounded, so the group is never skipped.
@compute @workgroup_size(256)
fn group_bounds(@builtin(local_invocation_index) lane: u32, @builtin(workgroup_id) wg: vec3u) {
    let g = params.bounds_first + wg.x;
    let i = g * 256u + lane;
    var lo = vec3f(3.0e38);
    var hi = vec3f(-3.0e38);
    if (i < arrayLength(&centers) / 3u && (bitcast<u32>(centers[i * 3u]) & 0x7f800000u) != 0x7f800000u) {
        let y = bitcast<u32>(centers[i * 3u + 1u]) & 0x7f800000u;
        let z = bitcast<u32>(centers[i * 3u + 2u]) & 0x7f800000u;
        if (y == 0x7f800000u || z == 0x7f800000u) {
            lo = vec3f(-3.0e38);
            hi = vec3f(3.0e38);
        } else {
            lo = vec3f(centers[i * 3u], centers[i * 3u + 1u], centers[i * 3u + 2u]);
            hi = lo;
        }
    }
    bound_lo[lane] = lo;
    bound_hi[lane] = hi;
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        workgroupBarrier();
        if (lane < stride) {
            bound_lo[lane] = min(bound_lo[lane], bound_lo[lane + stride]);
            bound_hi[lane] = max(bound_hi[lane], bound_hi[lane + stride]);
        }
    }
    if (lane == 0u) {
        store_bounds(g, bound_lo[0], bound_hi[0]);
    }
}

// True when every centre in group `g`'s box fails the coarse view test of `write_key` through one plane (behind the
// camera, or past CULL_MARGIN on one side), so each of its splats would be culled. A plane's test is linear, so it
// holds on the whole box when it holds at all eight corners; each corner must clear the plane by a margin far above
// the rounding of the per-splat evaluation (1e-5 of the summed magnitudes of the terms), so no splat inside can round
// the other way.
fn group_outside(g: u32) -> bool {
    let lo = load_bound(g, 0u);
    let hi = load_bound(g, 1u);
    if (any(lo > hi)) {
        return true;
    }
    let m = params.clip;
    let a = mat4x4f(abs(m[0]), abs(m[1]), abs(m[2]), abs(m[3]));
    var behind = true;
    var right = true;
    var left = true;
    var above = true;
    var below = true;
    for (var k = 0u; k < 8u; k++) {
        let c = select(lo, hi, vec3<bool>((k & 1u) != 0u, (k & 2u) != 0u, (k & 4u) != 0u));
        let p = m * vec4f(c, 1.0);
        let s = (a * vec4f(abs(c), 1.0)) * 1e-5;
        behind = behind && p.w < -s.w;
        right = right && p.x - CULL_MARGIN * p.w > s.x + CULL_MARGIN * s.w;
        left = left && -p.x - CULL_MARGIN * p.w > s.x + CULL_MARGIN * s.w;
        above = above && p.y - CULL_MARGIN * p.w > s.y + CULL_MARGIN * s.w;
        below = below && -p.y - CULL_MARGIN * p.w > s.y + CULL_MARGIN * s.w;
    }
    return behind || right || left || above || below;
}

var<workgroup> skip_group: u32;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) lane: u32,
        @builtin(workgroup_id) group: vec3u) {
    if (lane == 0u) {
        atomicStore(&local_visible, 0u);
        skip_group = select(0u, 1u, params.group_cull == 1u && group_outside(group.x));
    }
    // A group wholly outside the view reads no centres and writes no keys: nothing of it is in view, and `compact`
    // skips a group with nothing in view. Its shares are still written when this pass owes every slot's (mode 1).
    if (workgroupUniformLoad(&skip_group) == 1u) {
        if (params.fade_mode == 1u && id.x < params.count) {
            fade[id.x] = fade_share(id.x);
        }
        if (lane == 0u) {
            group_counts[group.x] = 0u;
        }
        return;
    }
    let shown = write_key(id.x);
    // All lanes issue the local atomic, including padding in the last group (adding zero).
    // This also avoids the partial-wave atomic issue seen on Adreno.
    atomicAdd(&local_visible, select(0u, 1u, shown));
    workgroupBarrier();
    if (lane == 0u) {
        let n = atomicLoad(&local_visible);
        group_counts[group.x] = n;
        atomicAdd(&visible, n);
    }
}

// Exclusive scan of the per-group counts, in place: afterwards `group_counts[g]` is the number of splats in view in
// groups before g, so group g's splats land at that offset. One workgroup; each thread sums a contiguous chunk.
var<workgroup> scan_sums: array<u32, 256>;

@compute @workgroup_size(256)
fn scan_groups(@builtin(local_invocation_index) lane: u32) {
    let groups = (params.count + 255u) / 256u;
    let chunk = (groups + 255u) / 256u;
    let first = min(lane * chunk, groups);
    let last = min(first + chunk, groups);
    var sum = 0u;
    for (var g = first; g < last; g++) { sum += group_counts[g]; }
    scan_sums[lane] = sum;
    workgroupBarrier();
    for (var offset = 1u; offset < 256u; offset <<= 1u) {
        var v = scan_sums[lane];
        if (lane >= offset) { v += scan_sums[lane - offset]; }
        workgroupBarrier();
        scan_sums[lane] = v;
        workgroupBarrier();
    }
    var running = scan_sums[lane] - sum;
    for (var g = first; g < last; g++) {
        let c = group_counts[g];
        group_counts[g] = running;
        running += c;
    }
}

// Stable stream compaction of the keys of the splats in view (slot order is kept, so equal keys keep the order the
// full sort would give them) together with their slot indices.
var<workgroup> flags: array<u32, 256>;
var<workgroup> group_empty: u32;

@compute @workgroup_size(256)
fn compact(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) lane: u32,
           @builtin(workgroup_id) group: vec3u) {
    // A group with nothing in view (its scanned offset equals the next group's, or the total for the last) has nothing
    // to move, and a group `main` skipped has stale keys, so stop before reading them.
    if (lane == 0u) {
        let groups = (params.count + 255u) / 256u;
        let next = select(atomicLoad(&visible), group_counts[group.x + 1u], group.x + 1u < groups);
        group_empty = select(0u, 1u, next == group_counts[group.x]);
    }
    if (workgroupUniformLoad(&group_empty) == 1u) {
        return;
    }
    var key = 0xffffffffu;
    if (id.x < params.count) { key = keys[id.x]; }
    let flag = select(0u, 1u, key != 0xffffffffu);
    flags[lane] = flag;
    workgroupBarrier();
    for (var offset = 1u; offset < 256u; offset <<= 1u) {
        var v = flags[lane];
        if (lane >= offset) { v += flags[lane - offset]; }
        workgroupBarrier();
        flags[lane] = v;
        workgroupBarrier();
    }
    if (flag == 1u) {
        let position = group_counts[group.x] + flags[lane] - 1u;
        compact_keys[position] = key;
        compact_values[position] = id.x;
    }
}

// Direct draws already cover the active prefix and reject culled slots in the
// vertex stage. They need the same keys and fades but no visibility count.
@compute @workgroup_size(256)
fn direct(@builtin(global_invocation_id) id: vec3u) {
    let ignored = write_key(id.x);
}
