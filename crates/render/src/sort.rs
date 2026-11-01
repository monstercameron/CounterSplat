//! GPU depth sort for splats, ported from the engine's `ComputeRadixSortMultipass` (LSD radix, 4-bit digits)
//! and its `PrefixSumKernel`. The radix and scan WGSL is supplied by `countersplat-shaders`.
//!
//! Each frame: a compute pass writes an order-preserving key per splat from its view depth. Five 4-bit
//! radix passes over the key's top 20 bits then sort (key, splat index) pairs ascending, leaving the indices
//! farthest-first. The final indices are copied into `splatOrder`. A frame whose view-model matrix is the same as
//! the last sorted one keeps that order and only repeats the copy.

use crate::camera::Mat4;
use countersplat_shaders::preprocess::{preprocess_source, Defines};
use std::cell::Cell;

const THREADS_PER_WORKGROUP: u32 = 256; // 16 × 16
const ELEMENTS_PER_THREAD: u32 = 8;
const ELEMENTS_PER_WORKGROUP: u32 = THREADS_PER_WORKGROUP * ELEMENTS_PER_THREAD;
const BUCKET_COUNT: u32 = 16;
const BITS_PER_PASS: u32 = 4;
/// Key bits the sort ignores. The key is an order-preserving float (sign, 8 exponent bits, 23 mantissa bits), so
/// dropping the low 12 leaves 11 mantissa bits: splats whose depths differ by under about 0.05% may swap places,
/// far below what blending shows. It takes five radix passes instead of eight.
const SKIPPED_KEY_BITS: u32 = 12;
const PASS_COUNT: u32 = (32 - SKIPPED_KEY_BITS) / BITS_PER_PASS;
/// The scan kernel's block size: two items per thread.
const ITEMS_PER_SCAN_WORKGROUP: u32 = THREADS_PER_WORKGROUP * 2;

const HISTOGRAM_SRC: &str = countersplat_shaders::radix::HISTOGRAM;
const REORDER_SRC: &str = countersplat_shaders::radix::REORDER;
const PREFIX_SUM_SRC: &str = countersplat_shaders::radix::PREFIX_SUM;
const DEPTH_KEYS_SRC: &str = include_str!("shaders/depth_keys.wgsl");
const DRAW_ARGS_SRC: &str = include_str!("shaders/draw_args.wgsl");

/// Bytes of the depth uniform: a `mat4x4f` followed by the splat count, padded to the WGSL struct size.
const DEPTH_UNIFORM_SIZE: u64 = 240;
/// Crossfading ranges the depth-key pass can hold.
const MAX_FADES: u32 = 1024;

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
    /// Splats the depth-key pass found in view, reset each sort.
    visible: wgpu::Buffer,
    args_pipeline: wgpu::ComputePipeline,
    args: wgpu::BindGroup,
    /// `draw_indirect` arguments covering the visible splats: written each sort.
    draw_args: wgpu::Buffer,
    /// Crossfading slot ranges (`set_fades`) and how many are in use.
    fades: wgpu::Buffer,
    fade_count: Cell<u32>,
    depth: wgpu::BindGroup,
    histogram_pipelines: Vec<wgpu::ComputePipeline>,
    reorder_pipelines: Vec<wgpu::ComputePipeline>,
    scan_pipeline: wgpu::ComputePipeline,
    add_pipeline: wgpu::ComputePipeline,
    passes: Vec<Pass>,
    scan_levels: Vec<ScanLevel>,
    /// Final sorted indices live in `values[0]` after the last pass.
    values: [wgpu::Buffer; 2],
    /// The `view_model` and `clip` the indices in `values[0]` were sorted for, bit for bit; `None` before the first sort.
    sorted_for: std::cell::RefCell<Vec<u8>>,
    last_output: std::cell::RefCell<Option<wgpu::Buffer>>,
}

impl DepthSorter {
    /// Moving Gaussian centres invalidate the cached order even with an unchanged camera/model.
    pub fn invalidate(&self) {
        self.sorted_for.borrow_mut().clear();
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
        assert!(count > 0, "sorting needs at least one splat");
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
        let keys = [
            storage("sort keys 0", key_bytes),
            storage("sort keys 1", key_bytes),
        ];
        let values = [
            storage("sort values 0", key_bytes),
            storage("sort values 1", key_bytes),
        ];
        let block_sums = storage("radix block sums", u64::from(BUCKET_COUNT * workgroups) * 4);
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
            ],
        );
        let visible = storage("visible splats", 4);
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
                crate::scene::INSTANCE_SIZE * 6,
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
                crate::scene::INSTANCE_SIZE * 6,
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
        let histogram_layout = layout(
            device,
            "histogram",
            &[
                storage_entry(0, true),
                storage_entry(1, false),
                uniform_entry(2),
            ],
        );
        let reorder_layout = layout(
            device,
            "reorder",
            &[
                storage_entry(0, true),
                storage_entry(1, false),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, false),
                uniform_entry(5),
            ],
        );
        let scan_layout = layout(
            device,
            "prefix sum",
            &[
                storage_entry(0, false),
                storage_entry(1, false),
                uniform_entry(2),
            ],
        );

        let depth_pipeline = pipeline(device, "depth keys", &depth_layout, DEPTH_KEYS_SRC, "main");
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
            ],
        );

        let mut histogram_pipelines = Vec::new();
        let mut reorder_pipelines = Vec::new();
        let mut passes = Vec::new();
        for pass in 0..PASS_COUNT {
            let bit = SKIPPED_KEY_BITS + pass * BITS_PER_PASS;
            let histogram_src = specialize(HISTOGRAM_SRC, &[("CURRENT_BIT", bit)]);
            let reorder_src = specialize(
                REORDER_SRC,
                &[
                    ("CURRENT_BIT", bit),
                    ("IS_FIRST_PASS", u32::from(pass == 0)),
                    ("IS_LAST_PASS", 0),
                ],
            );
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

            // Keys ping-pong from `keys_in`, then keys[0], keys[1], ...; values alternate so that the last pass
            // writes values[0], whatever the pass count.
            let key_src = if pass == 0 {
                &keys_in
            } else {
                &keys[((pass - 1) % 2) as usize]
            };
            let key_dst = &keys[(pass % 2) as usize];
            let value_src = &values[((pass + PASS_COUNT) % 2) as usize];
            let value_dst = &values[((pass + PASS_COUNT + 1) % 2) as usize];

            let histogram = bind(
                device,
                "radix histogram",
                &histogram_pipeline.get_bind_group_layout(0),
                &[
                    (0, key_src.as_entire_binding()),
                    (1, block_sums.as_entire_binding()),
                    (2, radix_uniform.as_entire_binding()),
                ],
            );
            let reorder = bind(
                device,
                "radix reorder",
                &reorder_pipeline.get_bind_group_layout(0),
                &[
                    (0, key_src.as_entire_binding()),
                    (1, key_dst.as_entire_binding()),
                    (2, block_sums.as_entire_binding()),
                    (3, value_src.as_entire_binding()),
                    (4, value_dst.as_entire_binding()),
                    (5, radix_uniform.as_entire_binding()),
                ],
            );
            histogram_pipelines.push(histogram_pipeline);
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
        let mut item_counts = vec![BUCKET_COUNT * workgroups];
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

        Self {
            metrics: Default::default(),
            count,
            active: Cell::new(count),
            radix_uniform,
            block_sums,
            depth_uniform,
            depth_pipeline,
            visible,
            args_pipeline,
            args,
            draw_args,
            fades,
            fade_count: Cell::new(0),
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
        self.sorted_for.borrow_mut().clear();
    }

    /// Slot ranges crossfading, sorted by start and not overlapping: (first slot, one past the last, share drawn).
    /// The depth-key pass writes each splat's share into the per-slot opacity buffer the splat shaders read, so a
    /// range fades smoothly; a range at share 0 is left out of the draw. Past `MAX_FADES` the rest are drawn whole. While any range fades, every frame
    /// sorts.
    pub fn set_fades(&self, queue: &wgpu::Queue, fades: &[(u32, u32, f32)]) {
        let fades = &fades[..fades.len().min(MAX_FADES as usize)];
        if fades.is_empty() && self.fade_count.get() == 0 {
            return;
        }
        let data: Vec<u32> = fades
            .iter()
            .flat_map(|&(start, end, share)| [start, end, share.to_bits(), 0])
            .collect();
        if !data.is_empty() {
            queue.write_buffer(&self.fades, 0, &u32s(&data));
        }
        self.fade_count.set(fades.len() as u32);
        self.sorted_for.borrow_mut().clear();
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

        let mut uniform = f32s(view_model);
        uniform.extend_from_slice(&f32s(clip));
        match cutout {
            Some((cut, view, model)) => {
                let target = crate::camera::transform_point(view, cut.target);
                uniform.extend_from_slice(&f32s(model));
                uniform.extend_from_slice(&f32s(&[
                    target[0],
                    target[1],
                    target[2],
                    cut.splat_reach,
                ]));
                uniform.extend_from_slice(&f32s(&[
                    cut.radius,
                    cut.floor_y,
                    cut.depth_margin,
                    cut.soft.max(1.001),
                ]));
            }
            None => uniform.extend_from_slice(&[0u8; 96]),
        }
        uniform.extend_from_slice(&active.to_le_bytes());
        uniform.extend_from_slice(&self.fade_count.get().to_le_bytes());
        // Nothing changed since the last sort (an unchanged camera and scene): the order is still right.
        if *self.sorted_for.borrow() == uniform {
            self.metrics.borrow_mut().count("cache_hits", 1);
            if self.last_output.borrow().as_ref() != Some(output) {
                encoder.copy_buffer_to_buffer(&self.values[0], 0, output, 0, bytes);
                *self.last_output.borrow_mut() = Some(output.clone());
            }
            return;
        }
        self.metrics.borrow_mut().count("sorts", 1);
        queue.write_buffer(&self.depth_uniform, 0, &uniform);
        *self.sorted_for.borrow_mut() = uniform;

        encoder.clear_buffer(&self.visible, 0, None);
        // Block sums past the active workgroups would hold a previous sort's counts; the scan reads all of them.
        encoder.clear_buffer(&self.block_sums, 0, None);
        let workgroups = active.div_ceil(ELEMENTS_PER_WORKGROUP).max(1);
        // Dispatches are ordered inside one compute pass. WebGPU resolves the storage read/write hazards at
        // each dispatch; ending a pass for every histogram/scan/reorder made dozens of Metal encoders per frame.
        let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("splat sort"),
            timestamp_writes: None,
        });
        dispatch(
            &mut compute,
            &self.depth_pipeline,
            &self.depth,
            active.div_ceil(THREADS_PER_WORKGROUP).max(1),
        );
        dispatch(&mut compute, &self.args_pipeline, &self.args, 1);
        for pass in 0..PASS_COUNT as usize {
            let bind_pass = &self.passes[pass];
            dispatch(
                &mut compute,
                &self.histogram_pipelines[pass],
                &bind_pass.histogram,
                workgroups,
            );
            for level in &self.scan_levels {
                dispatch(
                    &mut compute,
                    &self.scan_pipeline,
                    &level.scan,
                    level.workgroups,
                );
            }
            for level in self.scan_levels.iter().rev() {
                if let Some(add) = &level.add {
                    dispatch(&mut compute, &self.add_pipeline, add, level.workgroups);
                }
            }
            dispatch(
                &mut compute,
                &self.reorder_pipelines[pass],
                &bind_pass.reorder,
                workgroups,
            );
        }
        drop(compute);
        encoder.copy_buffer_to_buffer(&self.values[0], 0, output, 0, bytes);
        *self.last_output.borrow_mut() = Some(output.clone());
    }
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

/// Replaces the engine's `{NAME}` tokens and strips preprocessor blocks, as the engine's `Shader` does.
fn specialize(src: &str, tokens: &[(&str, u32)]) -> String {
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
    strip_preprocessor(&out)
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

fn f32s(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
