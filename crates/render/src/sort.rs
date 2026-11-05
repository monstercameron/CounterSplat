//! GPU depth sort for splats, ported from the engine's `ComputeRadixSortMultipass` (LSD radix, 4-bit digits)
//! and its `PrefixSumKernel`. The radix and scan WGSL is supplied by `countersplat-shaders`.
//!
//! Each frame: a compute pass writes an order-preserving key per splat from its view depth (16-bit logarithmic) and
//! compacts the splats in view. Four 4-bit radix passes then sort their (key, splat index) pairs ascending, leaving the indices
//! farthest-first. The final indices are copied into `splatOrder`. A frame whose view-model matrix is the same as
//! the last sorted one keeps that order and only repeats the copy.

use crate::camera::Mat4;
use countersplat_shaders::preprocess::{preprocess_source, Defines};
use std::cell::Cell;

const THREADS_PER_WORKGROUP: u32 = 256; // 16 × 16
const ELEMENTS_PER_THREAD: u32 = 8;
const ELEMENTS_PER_WORKGROUP: u32 = THREADS_PER_WORKGROUP * ELEMENTS_PER_THREAD;
/// The default radix digit width (`radix_bits`); 8-bit digits are an option under test.
const BITS_PER_PASS: u32 = 4;
/// Key bits the sort ignores. The depth-key pass writes a 16-bit logarithmic depth in the top half of the word
/// (`depth_keys.wgsl`): one step is 0.028% of the depth from 1 mm to 100 km, finer than the float key it replaced
/// (whose top 20 bits kept 11 mantissa bits, 0.05%), in four radix passes instead of five.
const SKIPPED_KEY_BITS: u32 = 16;
/// The scan kernel's block size: two items per thread.
const ITEMS_PER_SCAN_WORKGROUP: u32 = THREADS_PER_WORKGROUP * 2;

const HISTOGRAM_SRC: &str = countersplat_shaders::radix::HISTOGRAM;
const REORDER_SRC: &str = countersplat_shaders::radix::REORDER;
const PREFIX_SUM_SRC: &str = countersplat_shaders::radix::PREFIX_SUM;
const DEPTH_KEYS_SRC: &str = include_str!("shaders/depth_keys.wgsl");
const DRAW_ARGS_SRC: &str = include_str!("shaders/draw_args.wgsl");
const DEPTH_RANGE_SRC: &str = include_str!("shaders/depth_range.wgsl");
/// WGSL size of depth_range.wgsl's uniform.
const DEPTH_RANGE_UNIFORM_SIZE: u64 = 80;

/// WGSL depth uniform size, including alignment padding.
const DEPTH_UNIFORM_SIZE: u64 = 304;
/// Where `fade_mode` sits in the depth uniform: after everything the cached order depends on.
const FADE_MODE_OFFSET: usize = 288;
/// Where `bounds_first` and `group_cull` sit in the depth uniform.
const BOUNDS_FIRST_OFFSET: usize = 292;
const GROUP_CULL_OFFSET: usize = 296;
/// Where the footprint-cull fields start in the depth uniform (after the 232-byte payload and its padding).
const FOOTPRINT_OFFSET: usize = 240;
// Payload excludes the WGSL struct's final 8 bytes of alignment padding.
const DEPTH_UNIFORM_PAYLOAD: usize = 232;
/// Crossfading ranges the depth-key pass can hold.
const MAX_FADES: u32 = 8192;

/// Whether sorters created from now on compact the splats in view before the radix passes (the default). Off, they
/// sort every active slot as before; the picture is the same. A diagnostic switch for A/B timing in the browser
/// (`?sort_compaction=0`); natively `COUNTERSPLAT_SORT_COMPACTION=0` does the same.
static SORT_COMPACTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_sort_compaction(enabled: bool) {
    SORT_COMPACTION.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether sorts leave out the splats the vertex stage would discard for their footprint (the default; see
/// `footprint_culled` in `depth_keys.wgsl`). The picture is the same either way. A diagnostic switch for A/B timing
/// (`?footprint_cull=0` in the browser); natively `COUNTERSPLAT_FOOTPRINT_CULL=0` does the same.
static FOOTPRINT_CULL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_footprint_cull(enabled: bool) {
    FOOTPRINT_CULL.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

fn footprint_cull_enabled() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static ENV_OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ENV_OFF.get_or_init(|| std::env::var("COUNTERSPLAT_FOOTPRINT_CULL").is_ok_and(|v| v == "0")) {
            return false;
        }
    }
    FOOTPRINT_CULL.load(std::sync::atomic::Ordering::Relaxed)
}

/// The splats' scale texture and its codebook, as the scene's vertex stage reads them, for the footprint cull.
#[derive(Clone, Copy)]
pub struct SplatTextures<'a> {
    /// RGBA8 codebook indices of each splat's three log scales, `width` splats a row.
    pub scales: &'a wgpu::TextureView,
    /// The 256x1 RGBA32F SOG codebook; `.r` is the log scale.
    pub codebook: &'a wgpu::TextureView,
    pub width: u32,
}

/// What the vertex stage needs to size a splat's footprint this frame (see `scene`'s `viewport_size`,
/// `minPixelSize` and camera effects).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Footprint {
    /// The camera's projection (only its x and y scale are used).
    pub projection: Mat4,
    /// The viewport in pixels.
    pub viewport: [f32; 2],
    pub min_pixel_size: f32,
    /// Depth-of-field focus distance and blur scale in pixels (0: off), as in the scene's camera effects.
    pub focus_distance: f32,
    pub blur_scale: f32,
    /// Whether motion blur is on (it can stretch a footprint by up to 32 pixels).
    pub motion_blur: bool,
}

impl Footprint {
    /// The 48 bytes at `FOOTPRINT_OFFSET` of the depth uniform for `view_model`.
    fn uniform_bytes(&self, view_model: &Mat4, texture_width: u32) -> [u8; FADE_MODE_OFFSET - FOOTPRINT_OFFSET] {
        let [w, h] = self.viewport;
        // The vertex stage adds motion m m^T / 12 with |m| at most 32 pixels: at most 1024 / 12 to an eigenvalue.
        let motion = if self.motion_blur { 1024.0 / 12.0 * 1.02 } else { 0.0 };
        let values = [
            w * self.projection[0],
            h * self.projection[5],
            w,
            h,
            self.min_pixel_size,
            stretch_bound(view_model),
            self.focus_distance,
            self.blur_scale.max(0.0),
            motion,
            1.0,
            1024f32.min(w.min(h)),
            f32::from_bits(texture_width),
        ];
        let mut bytes = [0u8; FADE_MODE_OFFSET - FOOTPRINT_OFFSET];
        for (dst, value) in bytes.chunks_exact_mut(4).zip(values) {
            dst.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

/// An upper bound on |W x|^2 / |x|^2 for the 3x3 part W of `m`: the largest eigenvalue of W^T W, by power iteration
/// from a start that cannot be orthogonal to every eigenvector, raised by 1% plus a tiny absolute margin. For a
/// rigid view and a uniformly scaled model it is the scale squared.
fn stretch_bound(m: &Mat4) -> f32 {
    let w = |r: usize, c: usize| f64::from(m[c * 4 + r]);
    let mut g = [[0f64; 3]; 3];
    for (i, row) in g.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = (0..3).map(|k| w(k, i) * w(k, j)).sum();
        }
    }
    // Gershgorin: no eigenvalue exceeds the largest absolute row sum, which also bounds the iteration's estimate.
    let gershgorin = g.iter().map(|row| row.iter().map(|v| v.abs()).sum::<f64>()).fold(0.0, f64::max);
    let mut x = [1.0, 0.7548776662, 0.5698402910];
    let mut estimate = 0.0;
    for _ in 0..64 {
        let y = [0, 1, 2].map(|i| (0..3).map(|j| g[i][j] * x[j]).sum::<f64>());
        let norm = y.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm == 0.0 {
            break;
        }
        estimate = norm / x.iter().map(|v| v * v).sum::<f64>().sqrt();
        x = y.map(|v| v / norm);
    }
    // Power iteration converges from below; Gershgorin is a guaranteed (looser) bound to fall back on if it is slow.
    let tight = estimate * 1.01 + 1e-9;
    (if tight.is_finite() { tight.min(gershgorin * 1.0001 + 1e-9) } else { gershgorin }) as f32
}

/// Radix digit width for sorters created from now on: 4 (16 buckets, four passes over the 16-bit key) or 8 (256
/// buckets, two passes). Both give the same order. Natively `COUNTERSPLAT_RADIX_BITS` overrides it.
static RADIX_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(BITS_PER_PASS);

pub fn set_radix_bits(bits: u32) {
    assert!(bits == 4 || bits == 8, "radix digits are 4 or 8 bits");
    RADIX_BITS.store(bits, std::sync::atomic::Ordering::Relaxed);
}

fn radix_bits() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(bits) = std::env::var("COUNTERSPLAT_RADIX_BITS") {
        if let Ok(bits @ (4 | 8)) = bits.parse::<u32>() {
            return bits;
        }
    }
    RADIX_BITS.load(std::sync::atomic::Ordering::Relaxed)
}

/// A hole cut through the scene to keep a target visible: splats between the camera and `target` (world space),
/// within `radius` metres of it on screen, are culled; the road (world height below `floor_y`) is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cutout {
    pub target: [f32; 3],
    pub radius: f32,
    pub floor_y: f32,
    /// How far in front of the target a splat must be to be cut, so geometry beside the target is left alone.
    pub depth_margin: f32,
    /// The edge dissolves from `radius` to this many times it.
    pub soft: f32,
    /// How far a splat reaches past its centre (metres); near the camera that widens the hole so close splats
    /// whose centres fall just outside it do not cover it.
    pub splat_reach: f32,
}

/// One level of the recursive exclusive scan over the block histograms.
struct ScanLevel {
    workgroups: u32,
    scan: wgpu::BindGroup,
    /// Adds the next level's scanned block sums back into this level. Absent when the level is one workgroup.
    add: Option<wgpu::BindGroup>,
}

struct Pass {
    histogram: wgpu::BindGroup,
    reorder: wgpu::BindGroup,
}

/// Sorts splat indices by view depth on the GPU.
pub struct DepthSorter {
    metrics: std::cell::RefCell<crate::metrics::Metrics>,
    count: u32,
    /// Splats sorted: the first `active` of `count` (a streaming pool sorts only up to its highest used slot).
    active: Cell<u32>,
    radix_uniform: wgpu::Buffer,
    block_sums: wgpu::Buffer,
    depth_uniform: wgpu::Buffer,
    depth_pipeline: wgpu::ComputePipeline,
    /// Whether the caller needs a visibility count and indirect draw arguments.
    count_visible: bool,
    /// Whether the splats in view are compacted before the radix passes (count_visible, unless the
    /// COUNTERSPLAT_SORT_COMPACTION=0 diagnostic switch is set): then only they are sorted.
    compact: bool,
    /// Splats the depth-key pass found in view, reset each indirect sort.
    visible: wgpu::Buffer,
    args_pipeline: wgpu::ComputePipeline,
    /// Stable compaction of the splats in view ahead of the radix sort (indirect sorts only).
    scan_groups_pipeline: wgpu::ComputePipeline,
    compact_pipeline: wgpu::ComputePipeline,
    args: wgpu::BindGroup,
    /// `draw_indirect` arguments covering the visible splats: written each sort.
    draw_args: wgpu::Buffer,
    /// Crossfading slot ranges (`set_fades`) and how many are in use.
    fades: wgpu::Buffer,
    fade_count: Cell<u32>,
    /// Only the crossfade shares changed since the last sort (see set_fades): rewrite them, keep the order.
    fades_dirty: Cell<bool>,
    /// The key pass without visibility counting, used to rewrite fade shares alone.
    fade_pipeline: wgpu::ComputePipeline,
    /// Last uploaded fade bytes, also reused as the next upload buffer.
    fade_data: std::cell::RefCell<Vec<u8>>,
    depth: wgpu::BindGroup,
    histogram_pipelines: Vec<wgpu::ComputePipeline>,
    reorder_pipelines: Vec<wgpu::ComputePipeline>,
    scan_pipeline: wgpu::ComputePipeline,
    add_pipeline: wgpu::ComputePipeline,
    passes: Vec<Pass>,
    scan_levels: Vec<ScanLevel>,
    /// Final sorted indices live in `values[0]` after the last pass.
    values: [wgpu::Buffer; 2],
    /// Exact depth/cull inputs for the cached index order; None before the first sort.
    sorted_for: std::cell::RefCell<Option<[u8; FADE_MODE_OFFSET]>>,
    /// Whether `fade` holds every slot's current share (a compacting sort's mode-1 key pass wrote them and `set_fades`
    /// has not changed them since), so later sorts can skip the fade writes (`fade_mode` 2 in `depth_keys.wgsl`).
    fades_written: Cell<bool>,
    /// The `fade_mode` last written to the depth uniform (u32::MAX before the first).
    last_fade_mode: Cell<u32>,
    /// Whether a compacting sorter skips unchanged fade writes (see `write_fade_mode`).
    fade_skip: bool,
    /// Recomputes the centre boxes of a range of 256-slot groups (`group_bounds`).
    bounds_pipeline: wgpu::ComputePipeline,
    /// Slots (start, end) whose centres changed since their groups' boxes were last computed.
    bounds_dirty: Cell<Option<(u32, u32)>>,
    /// Groups the next key stage recomputes first: (first group, group count).
    pending_bounds: Cell<Option<(u32, u32)>>,
    /// Whether the key pass may skip groups wholly outside the view (compacting sorts; see `group_outside`).
    group_cull: bool,
    /// The splat textures' width when the key pass can read the splats' scales (`SplatTextures`), for the footprint cull.
    footprint_width: Option<u32>,
    /// This frame's vertex-stage footprint parameters (`set_footprint`); None leaves the cull off.
    footprint: Cell<Option<Footprint>>,
    last_output: std::cell::RefCell<Option<wgpu::Buffer>>,
    /// The depth prepass's share of the order (sorters that count the splats in view only).
    depth_range: Option<DepthRange>,
}

/// Which part of the sorted order the depth prepass draws (`depth_range.wgsl`).
struct DepthRange {
    pipeline: wgpu::ComputePipeline,
    /// Every splat in view: recorded after each sort, so a prepass without a limit draws everything.
    full: wgpu::BindGroup,
    /// Only the splats no farther than a depth (`limit_uniform`).
    limited: wgpu::BindGroup,
    limit_uniform: wgpu::Buffer,
    start: wgpu::Buffer,
    args: wgpu::Buffer,
}

impl DepthSorter {
    /// Moving Gaussian centres invalidate the cached order even with an unchanged camera/model.
    pub fn invalidate(&self) {
        *self.sorted_for.borrow_mut() = None;
        self.invalidate_slots(0, self.count);
    }

    /// `count` slots from `start` got new centres (an upload, move or release): sort again, and refresh the bounds
    /// of the 256-slot groups they belong to before the next key pass.
    pub fn invalidate_slots(&self, start: u32, count: u32) {
        *self.sorted_for.borrow_mut() = None;
        let end = start.saturating_add(count).min(self.count);
        if start >= end {
            return;
        }
        let dirty = match self.bounds_dirty.get() {
            Some((a, b)) => (a.min(start), b.max(end)),
            None => (start, end),
        };
        self.bounds_dirty.set(Some(dirty));
    }
    /// Builds every pipeline and bind group for `count` splats whose centres are in `centers`
    /// (`vec3f` per splat, tightly packed as `array<f32>`).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: u32,
        centers: &wgpu::Buffer,
        fade: &wgpu::Buffer,
    ) -> Self {
        Self::with_visibility_count(device, queue, count, centers, fade, true, None)
    }

    /// `new`, with the splats' scale texture and codebook, so the key pass can also cull the splats the vertex stage
    /// would discard for their footprint (`set_footprint`).
    pub fn with_splat_textures(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: u32,
        centers: &wgpu::Buffer,
        fade: &wgpu::Buffer,
        textures: SplatTextures<'_>,
    ) -> Self {
        Self::with_visibility_count(device, queue, count, centers, fade, true, Some(textures))
    }

    /// The vertex-stage parameters for this frame's footprint cull, read by the next `encode`. None (the default)
    /// keeps every splat the coarse view test keeps. A sorter without splat textures ignores it.
    pub fn set_footprint(&self, footprint: Option<Footprint>) {
        self.footprint.set(footprint);
    }

    /// A sorter that skips the GPU visibility count and draw arguments. The scene now draws through those
    /// arguments, so only tests that compare the two key passes still build this form.
    #[cfg(test)]
    pub(crate) fn new_for_direct_draw(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: u32,
        centers: &wgpu::Buffer,
        fade: &wgpu::Buffer,
    ) -> Self {
        Self::with_visibility_count(device, queue, count, centers, fade, false, None)
    }

    /// `count_visible`: count the splats in view on the GPU, compact them before the radix passes and write indirect
    /// draw arguments (the draw must then use `draw_args`); otherwise sort every active slot and draw them all.
    pub(crate) fn with_visibility_count(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: u32,
        centers: &wgpu::Buffer,
        fade: &wgpu::Buffer,
        count_visible: bool,
        splat_textures: Option<SplatTextures<'_>>,
    ) -> Self {
        assert!(count > 0, "sorting needs at least one splat");
        let compact = count_visible && SORT_COMPACTION.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(target_arch = "wasm32"))]
        let compact = compact
            && std::env::var("COUNTERSPLAT_SORT_COMPACTION").map_or(true, |value| value != "0");
        let workgroups = count.div_ceil(ELEMENTS_PER_WORKGROUP);
        let storage = |label: &str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(4),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniform = |label: &str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let key_bytes = u64::from(count) * 4;
        let keys_in = storage("depth keys", key_bytes);
        // The depth keys are only read by the first radix pass and rewritten by the next sort, so they are free
        // scratch afterwards: they serve as the second ping-pong buffer instead of a buffer of their own.
        let keys = [storage("sort keys 0", key_bytes), keys_in.clone()];
        let values = [
            storage("sort values 0", key_bytes),
            storage("sort values 1", key_bytes),
        ];
        let bits_per_pass = radix_bits();
        let bucket_count = 1u32 << bits_per_pass;
        let pass_count = (32 - SKIPPED_KEY_BITS) / bits_per_pass;
        let block_sums = storage("radix block sums", u64::from(bucket_count * workgroups) * 4);
        let depth_uniform = uniform("depth uniform", DEPTH_UNIFORM_SIZE);
        let radix_uniform = uniform("radix uniform", 8);
        queue.write_buffer(&radix_uniform, 0, &u32s(&[workgroups, count]));

        let depth_layout = layout(
            device,
            "depth",
            &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, true),
                storage_entry(5, false),
                storage_entry(6, false),
                storage_entry(7, false),
                storage_entry(8, false),
                texture_entry(9),
                texture_entry(10),
            ],
        );
        // Without the splats' textures the footprint cull stays off, and the key pass binds 1x1 stand-ins.
        let stand_ins = splat_textures.is_none().then(|| {
            let texture = |label, format| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            (
                texture("no splat scales", wgpu::TextureFormat::Rgba8Unorm),
                texture("no scale codebook", wgpu::TextureFormat::Rgba32Float),
            )
        });
        let (scales_view, codebook_view) = match (&splat_textures, &stand_ins) {
            (Some(textures), _) => (textures.scales, textures.codebook),
            (None, Some((scales, codebook))) => (scales, codebook),
            (None, None) => unreachable!("stand-ins exist whenever textures do not"),
        };
        let visible = storage("visible splats", 4);
        // Splats in view per 256-slot group, scanned into output offsets by scan_groups (indirect sorts only),
        // followed by each group's box of centres (six f32 words; see `bounds_base` in depth_keys.wgsl).
        let group_counts = storage(
            "visible per group",
            if count_visible {
                u64::from(count.div_ceil(THREADS_PER_WORKGROUP)) * (4 + 24)
            } else {
                4
            },
        );
        let fades = storage("splat fades", u64::from(MAX_FADES) * 16);
        let draw_args = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat draw args"),
            size: 20,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Until the first sort, draw everything.
        queue.write_buffer(
            &draw_args,
            0,
            &u32s(&[
                crate::scene::INSTANCE_SIZE * crate::scene::INDICES_PER_SPLAT,
                count.div_ceil(crate::scene::INSTANCE_SIZE),
                0,
                0,
                0,
            ]),
        );
        let args_uniform = uniform("draw args uniform", 16);
        queue.write_buffer(
            &args_uniform,
            0,
            &u32s(&[
                crate::scene::INSTANCE_SIZE * crate::scene::INDICES_PER_SPLAT,
                crate::scene::INSTANCE_SIZE,
                0,
                0,
            ]),
        );
        let args_layout = layout(
            device,
            "draw args",
            &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
            ],
        );
        let args_pipeline = pipeline(device, "draw args", &args_layout, DRAW_ARGS_SRC, "drawArgs");
        let args = bind(
            device,
            "draw args",
            &args_pipeline.get_bind_group_layout(0),
            &[
                (0, args_uniform.as_entire_binding()),
                (1, visible.as_entire_binding()),
                (2, draw_args.as_entire_binding()),
            ],
        );
        let mut histogram_entries = vec![
            storage_entry(0, true),
            storage_entry(1, false),
            uniform_entry(2),
        ];
        let mut reorder_entries = vec![
            storage_entry(0, true),
            storage_entry(1, false),
            storage_entry(2, true),
            storage_entry(3, true),
            storage_entry(4, false),
            uniform_entry(5),
        ];
        if compact {
            // The element count comes from the GPU's own visible count (USE_INDIRECT_SORT).
            histogram_entries.push(storage_entry(3, true));
            reorder_entries.push(storage_entry(6, true));
        }
        let histogram_layout = layout(device, "histogram", &histogram_entries);
        let reorder_layout = layout(device, "reorder", &reorder_entries);
        let scan_layout = layout(
            device,
            "prefix sum",
            &[
                storage_entry(0, false),
                storage_entry(1, false),
                uniform_entry(2),
            ],
        );

        let depth_pipeline = pipeline(
            device,
            "depth keys",
            &depth_layout,
            DEPTH_KEYS_SRC,
            if count_visible { "main" } else { "direct" },
        );
        let fade_pipeline = pipeline(device, "fade shares", &depth_layout, DEPTH_KEYS_SRC, "direct");
        let depth = bind(
            device,
            "depth keys",
            &depth_pipeline.get_bind_group_layout(0),
            &[
                (0, depth_uniform.as_entire_binding()),
                (1, centers.as_entire_binding()),
                (2, keys_in.as_entire_binding()),
                (3, visible.as_entire_binding()),
                (4, fades.as_entire_binding()),
                (5, fade.as_entire_binding()),
                (6, group_counts.as_entire_binding()),
                // Compaction writes the sort's first-pass inputs: the second ping-pong key buffer and the value
                // buffer pass 0 reads.
                (7, keys[0].as_entire_binding()),
                (8, values[(pass_count % 2) as usize].as_entire_binding()),
                (9, wgpu::BindingResource::TextureView(scales_view)),
                (10, wgpu::BindingResource::TextureView(codebook_view)),
            ],
        );
        let bounds_pipeline = pipeline(device, "splat group bounds", &depth_layout, DEPTH_KEYS_SRC, "group_bounds");
        // Compacting sorts may skip groups wholly outside the view (COUNTERSPLAT_GROUP_CULL=0 natively turns it off).
        #[cfg(not(target_arch = "wasm32"))]
        let group_cull = compact && std::env::var("COUNTERSPLAT_GROUP_CULL").map_or(true, |v| v != "0");
        #[cfg(target_arch = "wasm32")]
        let group_cull = compact;
        queue.write_buffer(&depth_uniform, GROUP_CULL_OFFSET as u64, &u32::from(group_cull).to_le_bytes());
        let scan_groups_pipeline = pipeline(device, "scan visible groups", &depth_layout, DEPTH_KEYS_SRC, "scan_groups");
        let compact_pipeline = pipeline(device, "compact visible", &depth_layout, DEPTH_KEYS_SRC, "compact");

        let mut histogram_pipelines = Vec::new();
        let mut reorder_pipelines = Vec::new();
        let mut passes = Vec::new();
        // Compacting sorts let workgroups past the visible count stop early (`skip_empty_workgroups`);
        // COUNTERSPLAT_RADIX_SKIP=0 natively keeps every workgroup running, for A/B timing.
        #[cfg(not(target_arch = "wasm32"))]
        let skip_empty = compact && std::env::var("COUNTERSPLAT_RADIX_SKIP").map_or(true, |v| v != "0");
        #[cfg(target_arch = "wasm32")]
        let skip_empty = compact;
        let finish = |src: String| {
            widen_digits(if skip_empty { skip_empty_workgroups(src) } else { src }, bits_per_pass)
        };
        for pass in 0..pass_count {
            let bit = SKIPPED_KEY_BITS + pass * bits_per_pass;
            let histogram_src = finish(specialize_with(HISTOGRAM_SRC, &[("CURRENT_BIT", bit)], compact));
            let reorder_src = finish(specialize_with(
                REORDER_SRC,
                &[
                    ("CURRENT_BIT", bit),
                    // Compacted sorts start from (key, slot) pairs; direct ones start from the identity order.
                    ("IS_FIRST_PASS", u32::from(pass == 0 && !compact)),
                    // The next sort regenerates keys; only the final values are consumed.
                    ("IS_LAST_PASS", u32::from(pass + 1 == pass_count)),
                ],
                compact,
            ));
            let histogram_pipeline = pipeline(
                device,
                "radix histogram",
                &histogram_layout,
                &histogram_src,
                "main",
            );
            let reorder_pipeline = pipeline(
                device,
                "radix reorder",
                &reorder_layout,
                &reorder_src,
                "main",
            );

            // Keys ping-pong between keys[1] (the depth keys, which pass 0 reads) and keys[0]; values alternate so
            // that the last pass writes values[0], whatever the pass count.
            // A compacted sort starts from keys[0] (written by compact); a direct one from the depth keys in keys[1].
            let first = usize::from(!compact);
            let key_src = &keys[(pass as usize + first) % 2];
            let key_dst = &keys[(pass as usize + first + 1) % 2];
            let value_src = &values[((pass + pass_count) % 2) as usize];
            let value_dst = &values[((pass + pass_count + 1) % 2) as usize];

            let mut histogram_bindings = vec![
                (0, key_src.as_entire_binding()),
                (1, block_sums.as_entire_binding()),
                (2, radix_uniform.as_entire_binding()),
            ];
            let mut reorder_bindings = vec![
                (0, key_src.as_entire_binding()),
                (1, key_dst.as_entire_binding()),
                (2, block_sums.as_entire_binding()),
                (3, value_src.as_entire_binding()),
                (4, value_dst.as_entire_binding()),
                (5, radix_uniform.as_entire_binding()),
            ];
            if compact {
                histogram_bindings.push((3, visible.as_entire_binding()));
                reorder_bindings.push((6, visible.as_entire_binding()));
            }
            let histogram = bind(
                device,
                "radix histogram",
                &histogram_pipeline.get_bind_group_layout(0),
                &histogram_bindings,
            );
            let reorder = bind(
                device,
                "radix reorder",
                &reorder_pipeline.get_bind_group_layout(0),
                &reorder_bindings,
            );            histogram_pipelines.push(histogram_pipeline);
            reorder_pipelines.push(reorder_pipeline);
            passes.push(Pass { histogram, reorder });
        }

        let scan_pipeline = pipeline(
            device,
            "prefix scan",
            &scan_layout,
            &specialize_scan(),
            "reduce_downsweep",
        );
        let add_pipeline = pipeline(
            device,
            "prefix add",
            &scan_layout,
            &specialize_scan(),
            "add_block_sums",
        );

        // Recursive exclusive scan over the histograms: level i scans `items[i]` into `items[i + 1]`.
        let mut items = vec![block_sums.clone()];
        let mut item_counts = vec![bucket_count * workgroups];
        let mut scan_levels = Vec::new();
        loop {
            let level = scan_levels.len();
            let level_count = item_counts[level];
            let level_workgroups = level_count.div_ceil(ITEMS_PER_SCAN_WORKGROUP);
            let blocks = storage("prefix block sums", u64::from(level_workgroups) * 4);
            let level_uniform = uniform("prefix uniform", 16);
            queue.write_buffer(&level_uniform, 0, &u32s(&[level_count]));
            let entries = [
                (0, items[level].as_entire_binding()),
                (1, blocks.as_entire_binding()),
                (2, level_uniform.as_entire_binding()),
            ];
            let scan = bind(
                device,
                "prefix scan",
                &scan_pipeline.get_bind_group_layout(0),
                &entries,
            );
            let add = (level_workgroups > 1).then(|| {
                bind(
                    device,
                    "prefix add",
                    &add_pipeline.get_bind_group_layout(0),
                    &entries,
                )
            });
            scan_levels.push(ScanLevel {
                workgroups: level_workgroups,
                scan,
                add,
            });
            items.push(blocks);
            item_counts.push(level_workgroups);
            if level_workgroups <= 1 {
                break;
            }
        }
        // `items` holds the block-sum buffers; bind groups keep them alive, so the Vec can go.
        drop(items);

        let depth_range = count_visible.then(|| {
            let start = storage("depth prepass start", 4);
            let args = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("depth prepass draw args"),
                size: 20,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // The view matrix and depth (bytes 0..68) are written only for a limited range.
            let range_uniform = |enabled: u32| {
                let buffer = uniform("depth range uniform", DEPTH_RANGE_UNIFORM_SIZE);
                let mut bytes = vec![0u8; DEPTH_RANGE_UNIFORM_SIZE as usize];
                bytes[68..80].copy_from_slice(&u32s(&[
                    enabled,
                    crate::scene::INSTANCE_SIZE * crate::scene::INDICES_PER_SPLAT,
                    crate::scene::INSTANCE_SIZE,
                ]));
                queue.write_buffer(&buffer, 0, &bytes);
                buffer
            };
            let (full_uniform, limit_uniform) = (range_uniform(0), range_uniform(1));
            let range_layout = layout(
                device,
                "depth range",
                &[
                    uniform_entry(0),
                    storage_entry(1, true),
                    storage_entry(2, true),
                    storage_entry(3, true),
                    storage_entry(4, false),
                    storage_entry(5, false),
                ],
            );
            let pipeline = pipeline(device, "depth prepass range", &range_layout, DEPTH_RANGE_SRC, "main");
            let group = |uniform: &wgpu::Buffer| {
                bind(
                    device,
                    "depth prepass range",
                    &pipeline.get_bind_group_layout(0),
                    &[
                        (0, uniform.as_entire_binding()),
                        (1, centers.as_entire_binding()),
                        (2, values[0].as_entire_binding()),
                        (3, visible.as_entire_binding()),
                        (4, start.as_entire_binding()),
                        (5, args.as_entire_binding()),
                    ],
                )
            };
            let (full, limited) = (group(&full_uniform), group(&limit_uniform));
            DepthRange {
                pipeline,
                full,
                limited,
                limit_uniform,
                start,
                args,
            }
        });

        Self {
            metrics: Default::default(),
            count,
            active: Cell::new(count),
            radix_uniform,
            block_sums,
            depth_uniform,
            depth_pipeline,
            count_visible,
            compact,
            visible,
            args_pipeline,
            scan_groups_pipeline,
            compact_pipeline,
            args,
            draw_args,
            fades,
            fade_count: Cell::new(0),
            fades_dirty: Cell::new(false),
            fade_pipeline,
            fade_data: Default::default(),
            depth,
            histogram_pipelines,
            reorder_pipelines,
            scan_pipeline,
            add_pipeline,
            passes,
            scan_levels,
            values,
            sorted_for: Default::default(),
            last_output: Default::default(),
            depth_range,
            footprint_width: splat_textures.map(|textures| textures.width),
            footprint: Cell::new(None),
            fades_written: Cell::new(false),
            last_fade_mode: Cell::new(u32::MAX),
            bounds_pipeline,
            bounds_dirty: Cell::new(Some((0, count))),
            pending_bounds: Cell::new(None),
            group_cull,
            // COUNTERSPLAT_FADE_SKIP=0 natively writes every share on every sort, for A/B timing.
            #[cfg(not(target_arch = "wasm32"))]
            fade_skip: std::env::var("COUNTERSPLAT_FADE_SKIP").map_or(true, |v| v != "0"),
            #[cfg(target_arch = "wasm32")]
            fade_skip: true,
        }
    }

    /// Sorts only the first `active` splats from now on (at most the sorter's size). A streaming pool keeps its used
    /// slots low, so this skips the empty tail.
    pub fn set_active(&self, queue: &wgpu::Queue, active: u32) {
        let active = active.min(self.count);
        if active == self.active.get() {
            return;
        }
        self.active.set(active);
        queue.write_buffer(
            &self.radix_uniform,
            0,
            &u32s(&[active.div_ceil(ELEMENTS_PER_WORKGROUP).max(1), active]),
        );
        *self.sorted_for.borrow_mut() = None;
    }

    /// Slot ranges crossfading, sorted by start and not overlapping: (first slot, one past the last, share drawn).
    /// The depth-key pass writes each splat's share into the per-slot opacity buffer the splat shaders read, so a
    /// range fades smoothly; a range at share 0 is left out of the draw. Past `MAX_FADES` the rest are drawn whole.
    ///
    /// The shares change every frame of a crossfade, but the order depends only on which splats are drawn: when the
    /// ranges are the same and no share crosses zero, a still camera keeps its sort and only the shares are rewritten
    /// (`fade_updates` in the profile); anything else sorts again.
    pub fn set_fades(&self, queue: &wgpu::Queue, fades: &[(u32, u32, f32)]) {
        let fades = &fades[..fades.len().min(MAX_FADES as usize)];
        let mut data = self.fade_data.borrow_mut();
        let shares_only = same_ranges_and_visibility(&data, fades);
        if !update_fade_data(&mut data, fades) {
            return;
        }
        self.fades_written.set(false);
        if !data.is_empty() {
            queue.write_buffer(&self.fades, 0, &data);
        }
        self.fade_count.set(fades.len() as u32);
        if shares_only {
            self.fades_dirty.set(true);
        } else {
            *self.sorted_for.borrow_mut() = None;
        }
    }

    /// Whether this sorter compacts the splats in view. Its order is then defined only for that prefix, so it must be
    /// drawn through `draw_args`.
    pub fn counts_visible(&self) -> bool {
        self.count_visible
    }

    /// The GPU's count of splats in view after the last sort (one `u32`); meaningful only when `counts_visible`.
    pub fn visible_buffer(&self) -> &wgpu::Buffer {
        &self.visible
    }

    /// `draw_indirect` arguments for the splats in view (all of them before the first sort).
    pub fn draw_args(&self) -> &wgpu::Buffer {
        &self.draw_args
    }

    /// Records the sort for the camera `view_model` (`view * model`) and copies the sorted indices to `output`.
    ///
    /// When `view_model` is bit-identical to the one `values[0]` was last sorted for, the order cannot have changed,
    /// so the key and radix dispatches are skipped and only the copy is recorded (`output` may be a different
    /// buffer from last time, so the copy is not skipped). The caller must submit the previous encoder before reuse.
    ///
    /// `clip` (`projection * view * model`) culls splats whose centre is behind the camera or well outside the view:
    /// they sort last, and `draw_args` covers only the others. `None` keeps every splat.
    ///
    pub fn active_slots(&self) -> u32 {
        self.active.get()
    }
    pub fn reset_profile(&self) {
        self.metrics.borrow_mut().reset();
    }
    pub fn profile_json(&self) -> serde_json::Value {
        self.metrics.borrow().json()
    }
    /// `cutout`, with the camera's `view` and the scene's `model`, cuts the splats hiding the target (see `Cutout`).
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view_model: &Mat4,
        clip: Option<&Mat4>,
        cutout: Option<(&Cutout, &Mat4, &Mat4)>,
        output: &wgpu::Buffer,
    ) {
        // A matrix whose clip-space w is always 1 and x, y always 0: nothing is culled.
        const NO_CULL: Mat4 = [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let clip = clip.unwrap_or(&NO_CULL);
        let active = self.active.get();
        let bytes = u64::from(active.max(1)) * 4;
        // When `output` is the sorter's own result buffer (see `indices_buffer`) there is nothing to copy.
        let in_place = output == &self.values[0];

        let mut uniform = [0u8; FADE_MODE_OFFSET];
        uniform[..DEPTH_UNIFORM_PAYLOAD]
            .copy_from_slice(&pack_depth_uniform(view_model, clip, cutout, active, self.fade_count.get()));
        if let (Some(width), Some(footprint)) = (self.footprint_width, self.footprint.get()) {
            if footprint_cull_enabled() {
                uniform[FOOTPRINT_OFFSET..FADE_MODE_OFFSET].copy_from_slice(&footprint.uniform_bytes(view_model, width));
            }
        }
        // Nothing changed since the last sort (an unchanged camera and scene): the order is still right.
        if self.sorted_for.borrow().as_ref() == Some(&uniform) {
            if self.fades_dirty.replace(false) {
                // Only crossfade shares changed (see `set_fades`): rewrite them for the active slots with the same
                // key pass, without counting, compacting or sorting. The depth uniform on the GPU is still this one.
                self.metrics.borrow_mut().count("fade_updates", 1);
                self.write_fade_mode(queue);
                let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("splat fade shares"),
                    timestamp_writes: None,
                });
                dispatch(
                    &mut compute,
                    &self.fade_pipeline,
                    &self.depth,
                    active.div_ceil(THREADS_PER_WORKGROUP).max(1),
                );
            } else {
                self.metrics.borrow_mut().count("cache_hits", 1);
            }
            if !in_place && self.last_output.borrow().as_ref() != Some(output) {
                encoder.copy_buffer_to_buffer(&self.values[0], 0, output, 0, bytes);
                *self.last_output.borrow_mut() = Some(output.clone());
            }
            return;
        }
        self.fades_dirty.set(false);
        self.metrics.borrow_mut().count("sorts", 1);
        queue.write_buffer(&self.depth_uniform, 0, &uniform);
        self.write_fade_mode(queue);
        if let Some((start, end)) = self.bounds_dirty.take().filter(|_| self.group_cull) {
            let first = start / THREADS_PER_WORKGROUP;
            let groups = end.div_ceil(THREADS_PER_WORKGROUP) - first;
            queue.write_buffer(&self.depth_uniform, BOUNDS_FIRST_OFFSET as u64, &first.to_le_bytes());
            self.pending_bounds.set(Some((first, groups)));
        }
        *self.sorted_for.borrow_mut() = Some(uniform);

        if self.count_visible {
            encoder.clear_buffer(&self.visible, 0, None);
        }
        // Block sums past the active workgroups would hold a previous sort's counts; the scan reads all of them.
        encoder.clear_buffer(&self.block_sums, 0, None);
        // Dispatches are ordered inside one compute pass. WebGPU resolves the storage read/write hazards at
        // each dispatch; ending a pass for every histogram/scan/reorder made dozens of Metal encoders per frame.
        // Only a stage timer (`set_stage_timer`, a diagnostic) splits the pass, to place a timestamp after each stage.
        let stages = [SortStage::Keys, SortStage::Compact]
            .into_iter()
            .filter(|stage| self.compact || *stage != SortStage::Compact)
            .chain((0..self.passes.len() as u8).map(SortStage::Radix));
        match STAGE_TIMER.with(|timer| timer.borrow().clone()) {
            None => {
                let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("splat sort"),
                    timestamp_writes: None,
                });
                for stage in stages {
                    self.record_stage(&mut compute, stage, active);
                }
            }
            Some(timer) => {
                for stage in stages {
                    {
                        let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some("splat sort stage"),
                            timestamp_writes: None,
                        });
                        self.record_stage(&mut compute, stage, active);
                    }
                    timer.mark(encoder, stage.label());
                }
            }
        }
        if !in_place {
            encoder.copy_buffer_to_buffer(&self.values[0], 0, output, 0, bytes);
            *self.last_output.borrow_mut() = Some(output.clone());
        }
    }

    /// Records which part of the sorted order the depth prepass draws: with `limit` (`(view_model, far_depth)`), only
    /// the splats no farther than `far_depth` in view space, which are all a depth test of fragments no farther than
    /// that can see; without, every splat in view. Record it after `encode` each frame (the scene does), and again with
    /// a limit before a prepass that has one. Sorters that do not count the splats in view have no range.
    pub fn encode_depth_range(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, limit: Option<(&Mat4, f32)>) {
        let Some(range) = &self.depth_range else {
            return;
        };
        let group = match limit {
            Some((view_model, far_depth)) => {
                let mut bytes = Vec::with_capacity(68);
                bytes.extend(view_model.iter().flat_map(|v| v.to_le_bytes()));
                bytes.extend(far_depth.to_le_bytes());
                queue.write_buffer(&range.limit_uniform, 0, &bytes);
                &range.limited
            }
            None => &range.full,
        };
        let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("depth prepass range"),
            timestamp_writes: None,
        });
        dispatch(&mut compute, &range.pipeline, group, 1);
    }

    /// The first entry of the sorted order the depth prepass draws (one `u32`), when the sorter has a range.
    pub fn depth_start_buffer(&self) -> Option<&wgpu::Buffer> {
        self.depth_range.as_ref().map(|range| &range.start)
    }

    /// `draw_indexed_indirect` arguments for the depth prepass's range, when the sorter has one.
    pub fn depth_draw_args(&self) -> Option<&wgpu::Buffer> {
        self.depth_range.as_ref().map(|range| &range.args)
    }

    /// The buffer the sorted splat indices end up in (farthest first). Passing it as `encode`'s `output` skips the
    /// copy into a second buffer: the draw can read it directly. It holds the order only until the next sort.
    pub fn indices_buffer(&self) -> &wgpu::Buffer {
        &self.values[0]
    }
}

impl DepthSorter {
    /// Sets how the next key pass writes the per-slot fade shares (see `put_fade` in `depth_keys.wgsl`): direct
    /// sorters always write every slot (0 for culled ones, which they still draw); compacting sorters write every
    /// share once after `set_fades` changes them (1), then nothing (2) until they change again.
    fn write_fade_mode(&self, queue: &wgpu::Queue) {
        let mode: u32 = if !self.compact || !self.fade_skip {
            0
        } else if self.fades_written.replace(true) {
            2
        } else {
            1
        };
        if self.last_fade_mode.replace(mode) != mode {
            queue.write_buffer(&self.depth_uniform, FADE_MODE_OFFSET as u64, &mode.to_le_bytes());
        }
    }

    /// Records one stage of a sort into `compute` (see `SortStage`).
    fn record_stage(&self, compute: &mut wgpu::ComputePass<'_>, stage: SortStage, active: u32) {
        let workgroups = active.div_ceil(ELEMENTS_PER_WORKGROUP).max(1);
        match stage {
            SortStage::Keys => {
                if let Some((_, groups)) = self.pending_bounds.take() {
                    dispatch(compute, &self.bounds_pipeline, &self.depth, groups);
                }
                dispatch(compute, &self.depth_pipeline, &self.depth, active.div_ceil(THREADS_PER_WORKGROUP).max(1));
                if self.count_visible {
                    dispatch(compute, &self.args_pipeline, &self.args, 1);
                }
            }
            SortStage::Compact => {
                // Only the splats in view go through the radix passes (the rest sort last and are never drawn):
                // scan the per-group counts into output offsets, then compact keys and slots in slot order.
                dispatch(compute, &self.scan_groups_pipeline, &self.depth, 1);
                dispatch(compute, &self.compact_pipeline, &self.depth, active.div_ceil(THREADS_PER_WORKGROUP).max(1));
            }
            SortStage::Radix(pass) => {
                let pass = pass as usize;
                let bind_pass = &self.passes[pass];
                dispatch(compute, &self.histogram_pipelines[pass], &bind_pass.histogram, workgroups);
                for level in &self.scan_levels {
                    dispatch(compute, &self.scan_pipeline, &level.scan, level.workgroups);
                }
                for level in self.scan_levels.iter().rev() {
                    if let Some(add) = &level.add {
                        dispatch(compute, &self.add_pipeline, add, level.workgroups);
                    }
                }
                dispatch(compute, &self.reorder_pipelines[pass], &bind_pass.reorder, workgroups);
            }
        }
    }
}

/// The parts of a sort, in order: depth keys (with the visible count and draw arguments), compaction of the splats in
/// view, and each radix pass (histogram, scan, reorder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortStage {
    Keys,
    Compact,
    Radix(u8),
}

impl SortStage {
    fn label(self) -> &'static str {
        match self {
            SortStage::Keys => "sort keys",
            SortStage::Compact => "sort compact",
            SortStage::Radix(pass) => ["radix 0", "radix 1", "radix 2", "radix 3"][usize::from(pass).min(3)],
        }
    }
}

thread_local! {
    static STAGE_TIMER: std::cell::RefCell<Option<std::rc::Rc<crate::gpu_timing::GpuFrameTimer>>> =
        const { std::cell::RefCell::new(None) };
}

/// A diagnostic: while set, every sort encoded on this thread splits its compute pass per stage and marks `timer`
/// after each (`sort keys`, `sort compact`, `radix 0`..). The caller begins and ends the timer around the frame.
/// Splitting the pass costs a little, so the stages add up to slightly more than an unsplit sort.
pub fn set_stage_timer(timer: Option<std::rc::Rc<crate::gpu_timing::GpuFrameTimer>>) {
    STAGE_TIMER.with(|slot| *slot.borrow_mut() = timer);
}

fn dispatch(
    pass: &mut wgpu::ComputePass<'_>,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: u32,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(workgroups, 1, 1);
}

/// Replaces the engine's `{NAME}` tokens and strips preprocessor blocks, as the engine's `Shader` does. `indirect` selects the engine's `USE_INDIRECT_SORT` variant, which reads the number of elements
/// to sort from a storage buffer (the GPU's own visible count) instead of the uniform.
fn specialize_with(src: &str, tokens: &[(&str, u32)], indirect: bool) -> String {
    let mut out = src
        .replace("{WORKGROUP_SIZE_X}", "16")
        .replace("{WORKGROUP_SIZE_Y}", "16")
        .replace(
            "{THREADS_PER_WORKGROUP}",
            &THREADS_PER_WORKGROUP.to_string(),
        )
        .replace("{ELEMENTS_PER_THREAD}", &ELEMENTS_PER_THREAD.to_string());
    for (name, value) in tokens {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    let mut defines = Defines::new();
    if indirect {
        defines.insert("USE_INDIRECT_SORT".into(), String::new());
    }
    preprocess_source(&out, &mut defines)
        .unwrap_or_else(|e| panic!("preprocessing radix shader: {e:?}"))
}

/// Lets a workgroup of the indirect (compacting) histogram or reorder kernel that lies wholly past the GPU's count of
/// splats in view stop at once. The passes are dispatched for every active slot, so with a fraction of the slots in
/// view most workgroups used to run every round and barrier for nothing. The count is read once into workgroup memory
/// (`workgroupUniformLoad`), which keeps the early return in uniform control flow. A skipped histogram workgroup still
/// writes zero counts: its block-sum slots hold the previous pass's scanned offsets otherwise.
fn skip_empty_workgroups(src: String) -> String {
    const COUNT: &str = "let elementCount = sortElementCount[0];";
    const ENTRY: &str = "@compute @workgroup_size";
    assert!(src.contains(COUNT) && src.contains(ENTRY), "radix kernel pattern not found");
    let reorder = src.contains("digit_masks");
    let empty = if reorder {
        "return;".to_owned()
    } else {
        "if (TID < 16u && WORKGROUP_ID < uniforms.workgroupCount) {\n            block_sums[TID * uniforms.workgroupCount + WORKGROUP_ID] = 0u;\n        }\n        return;".to_owned()
    };
    src.replacen(ENTRY, &format!("var<workgroup> wg_element_count: u32;\n\n{ENTRY}"), 1).replace(
        COUNT,
        &format!(
            "if (TID == 0u) {{ wg_element_count = sortElementCount[0]; }}\n    let elementCount = workgroupUniformLoad(&wg_element_count);\n    if (WID >= elementCount) {{\n        {empty}\n    }}"
        ),
    )
}

/// Rewrites the engine's 4-bit histogram or reorder kernel for `bits`-wide digits. The engine hardcodes 16 buckets:
/// the digit mask, the bucket arrays (the reorder's ranking masks are 8 words a bucket, one bit a thread) and the
/// threads that own a bucket. With 256 buckets every thread owns one, and the mask clear becomes a strided loop.
/// Every pattern must be present, so a change in the upstream source fails here instead of sorting wrongly.
fn widen_digits(src: String, bits: u32) -> String {
    if bits == 4 {
        return src;
    }
    assert_eq!(bits, 8, "radix digits are 4 or 8 bits");
    let buckets = 1u32 << bits;
    assert!(buckets <= THREADS_PER_WORKGROUP, "each bucket needs a thread");
    let words = buckets * (THREADS_PER_WORKGROUP / 32);
    let reorder = src.contains("digit_masks");
    let mut out = src;
    let mut swap = |from: &str, to: &str| {
        assert!(out.contains(from), "radix kernel pattern not found: {from}");
        out = out.replace(from, to);
    };
    swap("& 0xFu", &format!("& {:#X}u", buckets - 1));
    swap("TID < 16u", &format!("TID < {buckets}u"));
    if reorder {
        swap("array<atomic<u32>, 128>", &format!("array<atomic<u32>, {words}>"));
        swap("array<u32, 16>", &format!("array<u32, {buckets}>"));
        swap(
            "if (TID < 128u) {\n        atomicStore(&digit_masks[TID], 0u);",
            &format!("for (var m = TID; m < {words}u; m += THREADS_PER_WORKGROUP) {{\n        atomicStore(&digit_masks[m], 0u);"),
        );
        swap("select(16u,", &format!("select({buckets}u,"));
    } else {
        swap("array<atomic<u32>, 16>", &format!("array<atomic<u32>, {buckets}>"));
    }
    out
}

/// The scan kernel only has the workgroup and item tokens, so it gets its own substitution.
fn specialize_scan() -> String {
    strip_preprocessor(
        &PREFIX_SUM_SRC
            .replace("{WORKGROUP_SIZE_X}", "16")
            .replace("{WORKGROUP_SIZE_Y}", "16")
            .replace(
                "{THREADS_PER_WORKGROUP}",
                &THREADS_PER_WORKGROUP.to_string(),
            )
            .replace(
                "{ITEMS_PER_WORKGROUP}",
                &ITEMS_PER_SCAN_WORKGROUP.to_string(),
            ),
    )
}

/// Runs the splat-wgsl preprocessor, so `#ifdef USE_INDIRECT_SORT` takes its `#else` branch (direct sort).
fn strip_preprocessor(src: &str) -> String {
    preprocess_source(src, &mut Defines::new())
        .unwrap_or_else(|e| panic!("preprocessing radix shader: {e:?}"))
}

fn pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    src: &str,
    entry: &str,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn layout(
    device: &wgpu::Device,
    label: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    })
}

fn bind(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    resources: &[(u32, wgpu::BindingResource)],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = resources
        .iter()
        .map(|(binding, resource)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

/// A 2D float texture read with `textureLoad` (unfilterable, so RGBA32F binds without the float32-filterable feature).
fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Whether `fades` has the same ranges as the uploaded `data` and every share stays on the same side of zero (a
/// share of 0, or one that is not a number, leaves the range out of the draw), so the sorted order still holds.
fn same_ranges_and_visibility(data: &[u8], fades: &[(u32, u32, f32)]) -> bool {
    data.len() == fades.len() * 16
        && data
            .chunks_exact(16)
            .zip(fades)
            .all(|(bytes, &(start, end, share))| {
                let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                let old = f32::from_bits(word(8));
                word(0) == start && word(4) == end && (old > 0.0) == (share > 0.0)
            })
}

// Keep the GPU's little-endian range bytes for change detection and reuse.
fn update_fade_data(data: &mut Vec<u8>, fades: &[(u32, u32, f32)]) -> bool {
    if data.len() == fades.len() * 16
        && data
            .chunks_exact(16)
            .zip(fades)
            .all(|(bytes, &(start, end, share))| {
                bytes[..4] == start.to_le_bytes()
                    && bytes[4..8] == end.to_le_bytes()
                    && bytes[8..12] == share.to_bits().to_le_bytes()
                    && bytes[12..16] == [0; 4]
            })
    {
        return false;
    }
    data.clear();
    data.reserve(fades.len() * 16);
    for &(start, end, share) in fades {
        data.extend_from_slice(&start.to_le_bytes());
        data.extend_from_slice(&end.to_le_bytes());
        data.extend_from_slice(&share.to_bits().to_le_bytes());
        data.extend_from_slice(&[0; 4]);
    }
    true
}

// Match the WGSL layout exactly without allocating temporary byte vectors
// for every matrix and cutout field on each submitted frame.
fn pack_depth_uniform(
    view_model: &Mat4,
    clip: &Mat4,
    cutout: Option<(&Cutout, &Mat4, &Mat4)>,
    active: u32,
    fade_count: u32,
) -> [u8; DEPTH_UNIFORM_PAYLOAD] {
    fn put_f32s(output: &mut [u8], offset: usize, values: &[f32]) {
        for (dst, value) in output[offset..offset + values.len() * 4]
            .chunks_exact_mut(4)
            .zip(values)
        {
            dst.copy_from_slice(&value.to_le_bytes());
        }
    }
    let mut bytes = [0; DEPTH_UNIFORM_PAYLOAD];
    put_f32s(&mut bytes, 0, view_model);
    put_f32s(&mut bytes, 64, clip);
    if let Some((cut, view, model)) = cutout {
        let target = crate::camera::transform_point(view, cut.target);
        put_f32s(&mut bytes, 128, model);
        put_f32s(
            &mut bytes,
            192,
            &[target[0], target[1], target[2], cut.splat_reach],
        );
        put_f32s(
            &mut bytes,
            208,
            &[
                cut.radius,
                cut.floor_y,
                cut.depth_margin,
                cut.soft.max(1.001),
            ],
        );
    }
    bytes[224..228].copy_from_slice(&active.to_le_bytes());
    bytes[228..232].copy_from_slice(&fade_count.to_le_bytes());
    bytes
}

#[cfg(test)]
mod uniform_tests {
    use super::*;

    // Recreate the previous final pass for paired GPU comparisons.
    fn use_final_key_write(sorter: &mut DepthSorter, device: &wgpu::Device) {
        let passes = sorter.passes.len() as u32;
        let bits = (32 - SKIPPED_KEY_BITS) / passes;
        let pass = passes - 1;
        let indirect = sorter.compact;
        let src = widen_digits(
            specialize_with(
                REORDER_SRC,
                &[
                    ("CURRENT_BIT", SKIPPED_KEY_BITS + pass * bits),
                    ("IS_FIRST_PASS", u32::from(pass == 0 && !indirect)),
                    ("IS_LAST_PASS", 0),
                ],
                indirect,
            ),
            bits,
        );
        let index = pass as usize;
        let layout = sorter.reorder_pipelines[index].get_bind_group_layout(0);
        sorter.reorder_pipelines[index] =
            pipeline(device, "reference final keys", &layout, &src, "main");
    }

    fn reference(
        view_model: &Mat4,
        clip: &Mat4,
        cutout: Option<(&Cutout, &Mat4, &Mat4)>,
        active: u32,
        fade_count: u32,
    ) -> Vec<u8> {
        fn f32s(values: &[f32]) -> Vec<u8> {
            values.iter().flat_map(|v| v.to_le_bytes()).collect()
        }
        let mut bytes = f32s(view_model);
        bytes.extend_from_slice(&f32s(clip));
        if let Some((cut, view, model)) = cutout {
            let target = crate::camera::transform_point(view, cut.target);
            bytes.extend_from_slice(&f32s(model));
            bytes.extend_from_slice(&f32s(&[target[0], target[1], target[2], cut.splat_reach]));
            bytes.extend_from_slice(&f32s(&[
                cut.radius,
                cut.floor_y,
                cut.depth_margin,
                cut.soft.max(1.001),
            ]));
        } else {
            bytes.extend_from_slice(&[0; 96]);
        }
        bytes.extend_from_slice(&active.to_le_bytes());
        bytes.extend_from_slice(&fade_count.to_le_bytes());
        bytes
    }

    #[test]
    fn direct_and_counted_depth_sorts_produce_identical_order_and_fades() {
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let count = 4099u32;
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let centers = buffer(
            "comparison centres",
            u64::from(count) * 12,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let points: Vec<[f32; 3]> = (0..count)
            .map(|i| {
                if i % 19 == 0 {
                    [f32::NAN; 3]
                } else {
                    [
                        (i % 25) as f32 * 0.2 - 2.4,
                        (i % 11) as f32 * 0.2 - 1.0,
                        -0.5 - (i % 1000) as f32 * 0.01,
                    ]
                }
            })
            .collect();
        queue.write_buffer(&centers, 0, bytemuck::cast_slice(&points));
        let fade_a = buffer(
            "counted fade",
            u64::from(count) * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let fade_b = buffer(
            "direct fade",
            u64::from(count) * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let order_a = buffer(
            "counted order",
            u64::from(count) * 4,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        let order_b = buffer(
            "direct order",
            u64::from(count) * 4,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        // Compare the old final-key write with the optimized direct sorter.
        let mut counted = DepthSorter::new(&device, &queue, count, &centers, &fade_a);
        use_final_key_write(&mut counted, &device);
        let direct = DepthSorter::new_for_direct_draw(&device, &queue, count, &centers, &fade_b);
        let matrix = crate::camera::IDENTITY;
        let cut = Cutout {
            target: [0.0, 0.0, -5.0],
            radius: 0.5,
            floor_y: -0.8,
            depth_margin: 0.2,
            soft: 1.5,
            splat_reach: 0.1,
        };
        let bytes = u64::from(count) * 4;
        for scenario in 0..3 {
            let ranges = if scenario == 0 {
                vec![]
            } else {
                vec![(11, 500, 0.0), (800, 1300, 0.4), (1800, 3000, 1.0)]
            };
            counted.set_fades(&queue, &ranges);
            direct.set_fades(&queue, &ranges);
            let active = if scenario == 2 { count - 111 } else { count };
            counted.set_active(&queue, active);
            direct.set_active(&queue, active);
            let clip = (scenario != 0).then_some(&matrix);
            let cutout = (scenario == 2).then_some((&cut, &matrix, &matrix));
            let readback = buffer(
                "sort comparison readback",
                bytes * 4 + 4,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            counted.encode(&queue, &mut encoder, &matrix, clip, cutout, &order_a);
            direct.encode(&queue, &mut encoder, &matrix, clip, cutout, &order_b);
            encoder.copy_buffer_to_buffer(&order_a, 0, &readback, 0, bytes);
            encoder.copy_buffer_to_buffer(&order_b, 0, &readback, bytes, bytes);
            encoder.copy_buffer_to_buffer(&fade_a, 0, &readback, bytes * 2, bytes);
            encoder.copy_buffer_to_buffer(&fade_b, 0, &readback, bytes * 3, bytes);
            encoder.copy_buffer_to_buffer(counted.visible_buffer(), 0, &readback, bytes * 4, 4);
            queue.submit(Some(encoder.finish()));
            let slice = readback.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let data = slice.get_mapped_range().unwrap();
            let n = bytes as usize;
            // The counted sort compacts the splats in view, so only that prefix is defined (the direct sort puts
            // the culled slots after it); its GPU count says how many are in view.
            let shown = u32::from_le_bytes(data[4 * n..4 * n + 4].try_into().unwrap()) as usize;
            assert!(shown > 0, "scenario {scenario} draws something");
            if let Some(at) = (0..shown).find(|&i| data[i * 4..i * 4 + 4] != data[n + i * 4..n + i * 4 + 4]) {
                let word = |o: usize, i: usize| u32::from_le_bytes(data[o + i * 4..o + i * 4 + 4].try_into().unwrap());
                panic!("scenario {scenario}: first difference at {at} of {shown}: counted {} direct {} (neighbours counted {:?} direct {:?})", word(0, at), word(n, at), (at.saturating_sub(2)..(at + 3).min(shown)).map(|i| word(0, i)).collect::<Vec<_>>(), (at.saturating_sub(2)..(at + 3).min(shown)).map(|i| word(n, i)).collect::<Vec<_>>());
            }
            assert_eq!(
                &data[..shown * 4],
                &data[n..n + shown * 4],
                "sort order scenario {scenario}"
            );
            // The direct sorter zeroes culled slots (it draws them); the compacting one keeps every slot's share (it
            // never draws a culled one). They must agree on every slot the direct sorter shows.
            let fade = |o: usize, i: usize| f32::from_le_bytes(data[o + i * 4..o + i * 4 + 4].try_into().unwrap());
            for i in 0..active as usize {
                let direct_share = fade(3 * n, i);
                if direct_share != 0.0 {
                    assert_eq!(fade(2 * n, i).to_bits(), direct_share.to_bits(), "fade of slot {i}, scenario {scenario}");
                }
            }
            let direct_shown = (0..active as usize).filter(|&i| fade(3 * n, i) != 0.0).count();
            assert_eq!(shown, direct_shown, "splats in view, scenario {scenario}");
        }
    }

    #[test]
    #[ignore = "manual GPU timestamp benchmark"]
    fn benchmark_final_radix_keys() {
        use crate::gpu_timing::{GpuFrameTimer, GpuTiming};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            return;
        };
        if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return;
        }
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            }))
        else {
            return;
        };
        let count = 2_550_000u32;
        let storage = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let buffer = |label: &str, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: storage,
                mapped_at_creation: false,
            })
        };
        let centers = buffer("sort benchmark centers", u64::from(count) * 12);
        let points: Vec<[f32; 3]> = (0..count)
            .map(|i| {
                [
                    ((i % 251) as f32 - 125.0) * 0.02,
                    0.0,
                    -1.0 - (i % 5000) as f32 * 0.01,
                ]
            })
            .collect();
        queue.write_buffer(&centers, 0, bytemuck::cast_slice(&points));
        let fades = [
            buffer("old fade", u64::from(count) * 4),
            buffer("new fade", u64::from(count) * 4),
        ];
        let outputs = [
            buffer("old order", u64::from(count) * 4),
            buffer("new order", u64::from(count) * 4),
        ];
        let mut old = DepthSorter::new_for_direct_draw(&device, &queue, count, &centers, &fades[0]);
        use_final_key_write(&mut old, &device);
        let sorters = [
            old,
            DepthSorter::new_for_direct_draw(&device, &queue, count, &centers, &fades[1]),
        ];
        let matrix = crate::camera::IDENTITY;
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..100 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                sorters[mode].invalidate();
                let timer = GpuFrameTimer::new(&device).unwrap();
                let mut encoder = device.create_command_encoder(&Default::default());
                assert!(timer.begin(&mut encoder, round));
                sorters[mode].encode(&queue, &mut encoder, &matrix, None, None, &outputs[mode]);
                assert!(timer.end(&mut encoder, "sort"));
                queue.submit(Some(encoder.finish()));
                assert!(timer.submitted(&queue));
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let GpuTiming::Complete { milliseconds, .. } = timer.timing() else {
                    panic!("GPU timer failed");
                };
                if round >= 10 {
                    samples[mode].push(milliseconds);
                }
            }
        }
        let mut paired: Vec<_> = samples[0]
            .iter()
            .zip(&samples[1])
            .map(|(old, new)| new - old)
            .collect();
        paired.sort_by(f64::total_cmp);
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        let mid = samples[0].len() / 2;
        println!(
            "full sort for {count} splats ({} pairs): previous {:.4} ms, skip final key write {:.4} ms; paired median delta {:.4} ms",
            samples[0].len(), samples[0][mid], samples[1][mid], paired[mid]
        );
    }

    #[test]
    #[ignore = "manual GPU timestamp benchmark"]
    fn benchmark_direct_depth_keys() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("no GPU adapter");
            return;
        };
        if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            eprintln!("GPU timestamp queries unavailable");
            return;
        }
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            }))
        else {
            eprintln!("timestamp device unavailable");
            return;
        };
        let count = 2_550_000u32;
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let centers = buffer("bench centres", u64::from(count) * 12, storage);
        let points: Vec<[f32; 3]> = (0..count)
            .map(|i| [((i % 13) as f32 - 6.0) * 0.5, 0.0, -2.0])
            .collect();
        queue.write_buffer(&centers, 0, bytemuck::cast_slice(&points));
        let keys = buffer("bench keys", u64::from(count) * 4, storage);
        let visible = buffer("bench visible", 4, storage);
        let fades = buffer("bench fades", 16, storage);
        let fade = buffer("bench fade", u64::from(count) * 4, storage);
        let uniform = buffer(
            "bench uniform",
            DEPTH_UNIFORM_SIZE,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let matrix = crate::camera::IDENTITY;
        queue.write_buffer(
            &uniform,
            0,
            &pack_depth_uniform(&matrix, &matrix, None, count, 0),
        );
        let layout = layout(
            &device,
            "bench depth",
            &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, true),
                storage_entry(5, false),
            ],
        );
        let pipelines = [
            pipeline(&device, "counted", &layout, DEPTH_KEYS_SRC, "main"),
            pipeline(&device, "direct", &layout, DEPTH_KEYS_SRC, "direct"),
        ];
        let group = bind(
            &device,
            "bench depth",
            &pipelines[0].get_bind_group_layout(0),
            &[
                (0, uniform.as_entire_binding()),
                (1, centers.as_entire_binding()),
                (2, keys.as_entire_binding()),
                (3, visible.as_entire_binding()),
                (4, fades.as_entire_binding()),
                (5, fade.as_entire_binding()),
            ],
        );
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("depth timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolved = buffer(
            "timestamp resolve",
            16,
            wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = buffer(
            "timestamp readback",
            16,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..13 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.clear_buffer(&visible, 0, None);
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("timed depth"),
                        timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                            query_set: &queries,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        }),
                    });
                    dispatch(
                        &mut pass,
                        &pipelines[mode],
                        &group,
                        count.div_ceil(THREADS_PER_WORKGROUP),
                    );
                }
                encoder.resolve_query_set(&queries, 0..2, &resolved, 0);
                encoder.copy_buffer_to_buffer(&resolved, 0, &readback, 0, 16);
                queue.submit(Some(encoder.finish()));
                let slice = readback.slice(..);
                let (tx, rx) = std::sync::mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |result| {
                    tx.send(result).unwrap();
                });
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                rx.recv().unwrap().unwrap();
                let data = slice.get_mapped_range().unwrap();
                let start = u64::from_le_bytes(data[0..8].try_into().unwrap());
                let end = u64::from_le_bytes(data[8..16].try_into().unwrap());
                if round >= 2 {
                    samples[mode].push(
                        (end - start) as f64 * f64::from(queue.get_timestamp_period())
                            / 1_000_000.0,
                    );
                }
                drop(data);
                readback.unmap();
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "depth keys for {count} splats: counted {:.3} ms, direct {:.3} ms",
            samples[0][5], samples[1][5]
        );
    }

    #[test]
    fn packed_uniform_matches_previous_bytes_with_and_without_cutouts() {
        let special = [
            0.0,
            -0.0,
            1.0,
            -123.5,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::from_bits(0x7fc0_1234),
            f32::MIN_POSITIVE,
        ];
        for seed in 0..32 {
            let view_model = std::array::from_fn(|i| special[(i + seed) % special.len()]);
            let clip = std::array::from_fn(|i| special[(i * 3 + seed) % special.len()]);
            let model = std::array::from_fn(|i| (i as f32 - seed as f32) * 0.03125);
            let cut = Cutout {
                target: [1.0, -2.0, 3.0],
                radius: 0.75,
                floor_y: -0.0,
                depth_margin: 0.4,
                soft: if seed % 2 == 0 { 0.5 } else { 2.5 },
                splat_reach: 0.12,
            };
            for cutout in [None, Some((&cut, &model, &model))] {
                let old = reference(
                    &view_model,
                    &clip,
                    cutout,
                    seed as u32,
                    u32::MAX - seed as u32,
                );
                let new = pack_depth_uniform(
                    &view_model,
                    &clip,
                    cutout,
                    seed as u32,
                    u32::MAX - seed as u32,
                );
                assert_eq!(old.as_slice(), new.as_slice());
            }
        }
    }

    #[test]
    fn fade_upload_reuses_identical_bytes_and_preserves_float_bits() {
        let mut bytes = Vec::new();
        assert!(!update_fade_data(&mut bytes, &[]));
        let ranges = [(3, 12, -0.0), (25, 30, f32::from_bits(0x7fc0_1234))];
        assert!(update_fade_data(&mut bytes, &ranges));
        let expected: Vec<u8> = ranges
            .iter()
            .flat_map(|&(start, end, share)| {
                [start, end, share.to_bits(), 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
            })
            .collect();
        assert_eq!(bytes, expected);
        let allocation = bytes.as_ptr();
        assert!(!update_fade_data(&mut bytes, &ranges));
        assert_eq!(bytes.as_ptr(), allocation);
        let changed = [(3, 12, 0.0), ranges[1]];
        assert!(
            update_fade_data(&mut bytes, &changed),
            "signed zero is bit-distinct"
        );
        let changed_nan = [changed[0], (25, 30, f32::from_bits(0x7fc0_1235))];
        assert!(
            update_fade_data(&mut bytes, &changed_nan),
            "NaN payloads are bit-distinct"
        );
        assert!(update_fade_data(&mut bytes, &[]));
        assert!(!update_fade_data(&mut bytes, &[]));
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_depth_uniform_packing() {
        use std::{hint::black_box, time::Instant};
        let matrix = crate::camera::IDENTITY;
        let cut = Cutout {
            target: [1.0, 2.0, -3.0],
            radius: 1.0,
            floor_y: 0.0,
            depth_margin: 0.1,
            soft: 1.5,
            splat_reach: 0.2,
        };
        for (scenario, cutout) in [("normal", None), ("cutout", Some((&cut, &matrix, &matrix)))] {
            let mut old_samples = Vec::new();
            let mut new_samples = Vec::new();
            for sample in 0..11 {
                for old in if sample % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let start = Instant::now();
                    for i in 0..300_000 {
                        if old {
                            black_box(reference(
                                black_box(&matrix),
                                black_box(&matrix),
                                black_box(cutout),
                                black_box(i),
                                2,
                            ));
                        } else {
                            black_box(pack_depth_uniform(
                                black_box(&matrix),
                                black_box(&matrix),
                                black_box(cutout),
                                black_box(i),
                                2,
                            ));
                        }
                    }
                    let ns = start.elapsed().as_nanos() as f64 / 300_000.0;
                    if old {
                        old_samples.push(ns);
                    } else {
                        new_samples.push(ns);
                    }
                }
            }
            old_samples.sort_by(f64::total_cmp);
            new_samples.sort_by(f64::total_cmp);
            println!(
                "{scenario} depth uniform: old {:.1} ns, new {:.1} ns",
                old_samples[5], new_samples[5]
            );
        }
    }
}
