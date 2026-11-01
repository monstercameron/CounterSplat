//! Uploads a decoded SOG bundle and draws it through the composed gsplat pipelines.
//!
//! Each frame the splats are sorted back-to-front by view depth on the GPU (`sort::DepthSorter`, a port of the
//! engine's radix sort). Each instance covers `INSTANCE_SIZE` splats, as the engine's
//! `GSplatResourceBase.instanceSize`, so the quad buffer holds that many six-vertex quads.
//!
//! A scene draws two ways: in colour (sorted, blended, depth tested but not written), and depth only (the engine's
//! `PREPASS_PASS` variant: fragments below `alphaClip` are discarded and the rest write depth). The frame uses the
//! depth pass to combine separately sorted scene layers when a host needs it.

use crate::camera::{self, Mat4};
#[cfg(not(target_arch = "wasm32"))]
use crate::env::Sky;
use crate::sog::{self, Bundle};
use crate::sort::DepthSorter;
#[cfg(not(target_arch = "wasm32"))]
use crate::DEPTH_FORMAT;
use crate::{bind_group_layouts, gsplat_depth_pipeline, gsplat_pipeline, shader_module};
use countersplat_shaders::binding::{Binding, BindingKind};
use countersplat_shaders::pipeline::{compose_pipeline, compose_pipeline_with};
use std::collections::BTreeMap;

mod data;
mod passes;

#[cfg(test)]
use data::centres_to_le_bytes;
use data::{
    centres_le_bytes, f32s, f32s_le_bytes, quad_vertices, u32s, upload_texture, write_slots,
};
pub use passes::{color_pass, depth_pass};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Splats per instance (`GSplatResourceBase.instanceSize` in the engine).
pub const INSTANCE_SIZE: u32 = 128;
// Reuse up to roughly 3 MiB of retired-centre data per scene. Unusually large
// releases use a temporary buffer so they do not permanently enlarge the cache.
const RELEASE_SCRATCH_SPLATS: usize = 262_144;
// The debug LOD label uses four bytes per slot. Large writes remain temporary.
const LOD_LABEL_SCRATCH_SPLATS: usize = 262_144;
// Compaction can reuse up to 3 MiB of GPU scratch per scene. Very large
// relocations remain temporary so they do not permanently grow the cache.
const COMPACTION_SCRATCH_BYTES: u64 = 3 * 1024 * 1024;

/// The engine's 32×32 RGBA8 blue-noise texture (`core/math/blue-noise.js`), row-major.
const BLUE_NOISE_32: &[u8] = include_bytes!("../assets/blue_noise_32_rgba8.bin");

/// Camera and viewport for one frame. `model` is the scene's splat-to-world matrix.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub model: Mat4,
    pub view: Mat4,
    pub projection: Mat4,
    pub width: u32,
    pub height: u32,
    pub near: f32,
    pub far: f32,
}

/// One composed pipeline with the binding table its bind groups follow.
struct Variant {
    pipeline: wgpu::RenderPipeline,
    layouts: Vec<wgpu::BindGroupLayout>,
    bindings: Vec<Binding>,
}

/// The colour and depth-prepass pipelines, compiled once and shared by every scene.
pub struct Pipelines {
    color: Variant,
    depth: Variant,
}

impl Pipelines {
    /// Composes and compiles both variants for `color_format`.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Result<Self> {
        let (vs, fs, bindings) = compose_pipeline().map_err(|e| format!("compose: {e:?}"))?;
        let (vs, fs) = (
            shader_module(device, "gsplat vs", &vs),
            shader_module(device, "gsplat fs", &fs),
        );
        let layouts = bind_group_layouts(device, &bindings);
        let color = Variant {
            pipeline: gsplat_pipeline(device, &vs, &fs, &layouts, color_format),
            layouts,
            bindings,
        };

        // Depth needs no colour, so the prepass skips the spherical-harmonics evaluation in the vertex stage.
        let (vs, fs, bindings) = compose_pipeline_with(&["PREPASS_PASS", "SH_BANDS=0"])
            .map_err(|e| format!("compose prepass: {e:?}"))?;
        let (vs, fs) = (
            shader_module(device, "gsplat depth vs", &vs),
            shader_module(device, "gsplat depth fs", &fs),
        );
        let layouts = bind_group_layouts(device, &bindings);
        let depth = Variant {
            pipeline: gsplat_depth_pipeline(device, &vs, &fs, &layouts),
            layouts,
            bindings,
        };
        Ok(Self { color, depth })
    }
}

/// A splat asset resident on the GPU, with its bind groups and sorter.
pub struct GsplatScene {
    device: wgpu::Device,
    color_groups: Vec<wgpu::BindGroup>,
    depth_groups: Vec<wgpu::BindGroup>,
    /// Uniform and storage buffers, keyed by the binding name from the tables.
    buffers: BTreeMap<String, wgpu::Buffer>,
    quad: wgpu::Buffer,
    quad_indices: wgpu::Buffer,
    count: u32,
    sorter: DepthSorter,
    /// Mean splat centre in splat space, for ordering several scenes back to front.
    pub centre: [f32; 3],
    /// Splat-space bounds of the splats between the 0.5th and 99.5th percentiles per axis, for screen-space culling.
    pub bounds: ([f32; 3], [f32; 3]),
    /// The splat textures by stream name, kept so a pool can be written in place.
    textures: BTreeMap<String, wgpu::Texture>,
    centers: wgpu::Buffer,
    release_scratch: std::cell::RefCell<Vec<[f32; 3]>>,
    lod_label_scratch: std::cell::RefCell<Vec<u32>>,
    compaction_scratch: std::cell::RefCell<Option<wgpu::Buffer>>,
    splat_width: u32,
    uniform_cache: std::cell::RefCell<BTreeMap<String, Vec<u8>>>,
    io_metrics: std::cell::RefCell<crate::metrics::Metrics>,
}

/// Uploads the bundle's textures, codebook and centres for `pipelines`. In the depth pass, fragments with alpha
/// below `depth_alpha_clip` are discarded, so it sets how solid a splat must be to hide what is behind it.
pub fn build(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    bundle: &Bundle,
    depth_alpha_clip: f32,
) -> Result<GsplatScene> {
    build_inner(device, queue, pipelines, bundle, depth_alpha_clip)
}

fn build_inner(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    bundle: &Bundle,
    depth_alpha_clip: f32,
) -> Result<GsplatScene> {
    let textures = bundle
        .textures
        .iter()
        .filter(|(name, _)| !matches!(**name, "means_l" | "means_u"))
        .map(|(name, tex)| {
            (
                name.to_string(),
                upload_texture(
                    device,
                    queue,
                    name,
                    tex.width,
                    tex.height,
                    wgpu::TextureFormat::Rgba8Unorm,
                    Some(&tex.rgba),
                ),
            )
        })
        .collect();
    let width = bundle.textures["means_l"].width;
    assemble(
        device,
        queue,
        pipelines,
        &bundle.meta,
        textures,
        &sog::centres(bundle),
        width,
        depth_alpha_clip,
    )
}

/// Width of a streaming pool's per-splat textures.
pub const POOL_WIDTH: u32 = 2048;
/// Most rows of a pool's SH centroid texture (64 centroids per row); the device's largest texture side may cap it.
pub const POOL_CENTROID_ROWS: u32 = 16384;

/// An empty splat pool of `capacity` slots for streaming (`stream::LevelStream` fills it). `meta` gives the shared
/// position range and codebooks every slot is encoded with; its count is ignored. Every slot starts empty: its
/// centre is NaN, which the sort culls.
pub fn pool(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    meta: &sog::Meta,
    capacity: u32,
    depth_alpha_clip: f32,
) -> Result<GsplatScene> {
    pool_with_centroid_rows(
        device,
        queue,
        pipelines,
        meta,
        capacity,
        depth_alpha_clip,
        POOL_CENTROID_ROWS,
    )
}

/// A pool with the same slot budget but no unused SH palette rows. `centroid_rows`
/// includes the reserved zero row; callers retain every reachable palette slot.
#[allow(clippy::too_many_arguments)]
pub fn pool_with_centroid_rows(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    meta: &sog::Meta,
    capacity: u32,
    depth_alpha_clip: f32,
    centroid_rows: u32,
) -> Result<GsplatScene> {
    let rows = capacity.div_ceil(POOL_WIDTH);
    let mut textures: BTreeMap<String, wgpu::Texture> = ["quats", "scales", "sh0", "sh_labels"]
        .iter()
        .map(|name| {
            (
                name.to_string(),
                upload_texture(
                    device,
                    queue,
                    name,
                    POOL_WIDTH,
                    rows,
                    wgpu::TextureFormat::Rgba8Unorm,
                    None,
                ),
            )
        })
        .collect();
    textures.insert(
        "sh_centroids".into(),
        upload_texture(
            device,
            queue,
            "sh_centroids",
            64 * 15,
            centroid_rows
                .clamp(1, POOL_CENTROID_ROWS)
                .min(device.limits().max_texture_dimension_2d),
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        ),
    );
    let meta = sog::Meta {
        count: rows * POOL_WIDTH,
        ..meta.clone()
    };
    let empty = vec![[f32::NAN; 3]; meta.count as usize];
    assemble(
        device,
        queue,
        pipelines,
        &meta,
        textures,
        &empty,
        POOL_WIDTH,
        depth_alpha_clip,
    )
}

/// The shared part of `build` and `pool`: codebook, samplers, buffers, sorter and bind groups around `textures`.
#[allow(clippy::too_many_arguments)]
fn assemble(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    meta: &sog::Meta,
    textures: BTreeMap<String, wgpu::Texture>,
    centres: &[[f32; 3]],
    splat_width: u32,
    depth_alpha_clip: f32,
) -> Result<GsplatScene> {
    let count = meta.count;
    let mut views: BTreeMap<String, wgpu::TextureView> = textures
        .iter()
        .map(|(name, t)| (name.clone(), t.create_view(&Default::default())))
        .collect();
    let codebook: Vec<u8> = sog::codebook_rgba32f(meta)
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    views.insert(
        "sogCodebook".into(),
        upload_texture(
            device,
            queue,
            "sogCodebook",
            256,
            1,
            wgpu::TextureFormat::Rgba32Float,
            Some(&codebook),
        )
        .create_view(&Default::default()),
    );
    views.insert(
        "blueNoiseTex32".into(),
        upload_texture(
            device,
            queue,
            "blueNoiseTex32",
            32,
            32,
            wgpu::TextureFormat::Rgba8Unorm,
            Some(BLUE_NOISE_32),
        )
        .create_view(&Default::default()),
    );
    let nearest = wgpu::SamplerDescriptor {
        label: Some("gsplat nearest"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    };
    let sampler = device.create_sampler(&nearest);
    // The blue-noise lookup wraps, as the engine's `getBlueNoiseTexture` sampler does.
    let blue_noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("blue noise repeat"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        ..nearest
    });

    // One buffer per uniform name across both tables, so the passes share the matrices.
    let mut buffers: BTreeMap<String, wgpu::Buffer> = BTreeMap::new();
    for b in pipelines
        .color
        .bindings
        .iter()
        .chain(&pipelines.depth.bindings)
    {
        if let BindingKind::Uniform { ty } = &b.kind {
            if buffers.contains_key(&b.name) {
                continue;
            }
            let size = if ty == "mat4x4f" { 64 } else { 16 };
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&b.name),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers.insert(b.name.clone(), buffer);
        }
    }
    let order = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatOrder"),
        size: u64::from(count.max(1)) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    buffers.insert("splatOrder".into(), order);

    let centers = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splat centres"),
        size: u64::from(count.max(1)) * 12,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&centers, 0, centres_le_bytes(centres).as_ref());
    // Per-slot opacity for crossfades (see the scene shader): whole until a sort writes otherwise.
    let fade = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatFade"),
        size: u64::from(count.max(1)) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &fade,
        0,
        f32s_le_bytes(&vec![1.0; count.max(1) as usize]).as_ref(),
    );
    let sorter = DepthSorter::new_for_direct_draw(device, queue, count, &centers, &fade);
    buffers.insert("splatFade".into(), fade);
    // Per-slot level of detail (plus one; 0 for splats not from a streamed level) and the LOD tint switch.
    let lod = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatLod"),
        size: u64::from(count.max(1)) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    buffers.insert("splatLod".into(), lod);
    let debug = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatDebug"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    buffers.insert("splatDebug".into(), debug);
    buffers.insert("splatCenters".into(), centers.clone());
    let scene_buffers = Buffers(&buffers);
    scene_buffers.set(queue, "splatTextureSize", &u32s(&[splat_width]));
    scene_buffers.set(queue, "numSplats", &u32s(&[count]));
    scene_buffers.set(queue, "minPixelSize", &f32s(&[2.0]));
    scene_buffers.set(queue, "alphaClipForward", &f32s(&[1.0 / 255.0]));
    scene_buffers.set(queue, "alphaClip", &f32s(&[depth_alpha_clip]));
    scene_buffers.set(queue, "blueNoiseJitter", &f32s(&[0.0, 0.0, 0.0, 0.0]));

    let groups = |variant: &Variant| -> Vec<wgpu::BindGroup> {
        variant
            .layouts
            .iter()
            .enumerate()
            .map(|(group, layout)| {
                let entries: Vec<wgpu::BindGroupEntry> = variant
                    .bindings
                    .iter()
                    .filter(|b| b.group == group as u32)
                    .map(|b| {
                        let resource = match &b.kind {
                            BindingKind::Uniform { .. } | BindingKind::Storage { .. } => {
                                buffers[&b.name].as_entire_binding()
                            }
                            BindingKind::Texture { .. } => wgpu::BindingResource::TextureView(
                                views.get(&b.name).expect("texture for binding"),
                            ),
                            BindingKind::Sampler { .. } if b.name.starts_with("blueNoise") => {
                                wgpu::BindingResource::Sampler(&blue_noise_sampler)
                            }
                            BindingKind::Sampler { .. } => wgpu::BindingResource::Sampler(&sampler),
                        };
                        wgpu::BindGroupEntry {
                            binding: b.binding,
                            resource,
                        }
                    })
                    .collect();
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("gsplat group {group}")),
                    layout,
                    entries: &entries,
                })
            })
            .collect()
    };
    let color_groups = groups(&pipelines.color);
    let depth_groups = groups(&pipelines.depth);

    let quad_data = quad_vertices();
    let quad = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gsplat quads"),
        size: quad_data.len() as u64 * 4,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&quad, 0, f32s_le_bytes(&quad_data).as_ref());
    // Reuse the two shared triangle corners: expensive SOG/SH/lighting runs four times per splat, not six.
    let indices: Vec<u16> = (0..INSTANCE_SIZE as u16)
        .flat_map(|slot| [0, 1, 2, 0, 2, 3].map(|i| slot * 4 + i))
        .collect();
    let quad_indices = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gsplat quad indices"),
        size: (indices.len() * 2) as u64,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &quad_indices,
        0,
        &indices
            .iter()
            .flat_map(|i| i.to_le_bytes())
            .collect::<Vec<_>>(),
    );

    Ok(GsplatScene {
        device: device.clone(),
        color_groups,
        depth_groups,
        buffers,
        quad,
        quad_indices,
        count,
        sorter,
        centre: sog::mean_centre(centres),
        bounds: robust_bounds(centres),
        textures,
        centers,
        release_scratch: Default::default(),
        lod_label_scratch: Default::default(),
        compaction_scratch: Default::default(),
        splat_width,
        uniform_cache: Default::default(),
        io_metrics: Default::default(),
    })
}

impl GsplatScene {
    /// Writes `centres.len()` splats into slots from `start`: one RGBA8 texel per splat for each named stream
    /// (`quats`, `scales`, `sh0`, `sh_labels`), tightly packed, and their shared decoded centres.
    pub fn write_splats(
        &self,
        queue: &wgpu::Queue,
        start: u32,
        streams: &[(&str, &[u8])],
        centres: &[[f32; 3]],
    ) {
        self.io_metrics
            .borrow_mut()
            .count("stream_splats_uploaded", centres.len() as u64);
        self.io_metrics.borrow_mut().count(
            "stream_payload_bytes",
            streams.iter().map(|(_, b)| b.len() as u64).sum::<u64>() + centres.len() as u64 * 12,
        );
        let count = centres.len() as u32;
        for (name, texels) in streams {
            if let Some(texture) = self.textures.get(*name) {
                write_slots(queue, texture, self.splat_width, start, count, texels);
            }
        }
        queue.write_buffer(
            &self.centers,
            u64::from(start) * 12,
            centres_le_bytes(centres).as_ref(),
        );
        self.sorter.invalidate();
    }

    // Queue submissions execute in order, so consecutive compactions can
    // safely share this buffer even while earlier commands are still in flight.
    fn compaction_buffer(&self, count: u32) -> wgpu::Buffer {
        let bytes = u64::from(count) * 12;
        debug_assert!(bytes > 0);
        let create = |size| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("stream compaction scratch"),
                size,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        if bytes > COMPACTION_SCRATCH_BYTES {
            return create(bytes);
        }
        let mut cached = self.compaction_scratch.borrow_mut();
        if cached.as_ref().is_none_or(|buffer| buffer.size() < bytes) {
            let capacity = bytes.next_power_of_two().min(COMPACTION_SCRATCH_BYTES);
            *cached = Some(create(capacity));
        }
        cached.as_ref().unwrap().clone()
    }

    /// Relocates a pool range through GPU scratch space, including overlapping ranges.
    /// The caller releases the vacated tail and updates its allocation bookkeeping.
    pub(crate) fn move_slots(&self, queue: &wgpu::Queue, from: u32, to: u32, count: u32) {
        if from == to || count == 0 {
            return;
        }
        debug_assert!(from + count <= self.count && to + count <= self.count);
        let scratch = self.compaction_buffer(count);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("stream compact"),
            });
        // Copy single rows so differently aligned source and destination ranges can share
        // the same tightly packed scratch buffer without row-stride padding.
        for name in ["quats", "scales", "sh0", "sh_labels"] {
            let texture = &self.textures[name];
            for (start, into_scratch) in [(from, true), (to, false)] {
                let (mut slot, mut offset) = (start, 0u32);
                while offset < count {
                    let width = (self.splat_width - slot % self.splat_width).min(count - offset);
                    let texture_copy = wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: slot % self.splat_width,
                            y: slot / self.splat_width,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    };
                    let buffer_copy = wgpu::TexelCopyBufferInfo {
                        buffer: &scratch,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: u64::from(offset) * 4,
                            bytes_per_row: None,
                            rows_per_image: None,
                        },
                    };
                    let extent = wgpu::Extent3d {
                        width,
                        height: 1,
                        depth_or_array_layers: 1,
                    };
                    if into_scratch {
                        encoder.copy_texture_to_buffer(texture_copy, buffer_copy, extent);
                    } else {
                        encoder.copy_buffer_to_texture(buffer_copy, texture_copy, extent);
                    }
                    slot += width;
                    offset += width;
                }
            }
        }
        encoder.copy_buffer_to_buffer(
            &self.centers,
            u64::from(from) * 12,
            &scratch,
            0,
            u64::from(count) * 12,
        );
        encoder.copy_buffer_to_buffer(
            &scratch,
            0,
            &self.centers,
            u64::from(to) * 12,
            u64::from(count) * 12,
        );
        queue.submit(Some(encoder.finish()));
        self.sorter.invalidate();
    }

    /// Empties `count` slots from `start`: their centres become NaN, which the sort culls.
    pub fn release(&self, queue: &wgpu::Queue, start: u32, count: u32) {
        self.io_metrics
            .borrow_mut()
            .count("stream_slots_released", u64::from(count));
        let count = count as usize;
        if count <= RELEASE_SCRATCH_SPLATS {
            let mut empty = self.release_scratch.borrow_mut();
            empty.resize(count, [f32::NAN; 3]);
            queue.write_buffer(
                &self.centers,
                u64::from(start) * 12,
                centres_le_bytes(&empty).as_ref(),
            );
        } else {
            let empty = vec![[f32::NAN; 3]; count];
            queue.write_buffer(
                &self.centers,
                u64::from(start) * 12,
                centres_le_bytes(&empty).as_ref(),
            );
        }
        self.sorter.invalidate();
    }

    /// Records the level of detail and leaf held by `count` slots from `start` (for the debug colours).
    pub fn write_lod(&self, queue: &wgpu::Queue, start: u32, count: u32, lod: usize, leaf: usize) {
        let value = (leaf as u32) << 4 | (lod as u32 + 1).min(15);
        with_lod_label_bytes(
            &mut self.lod_label_scratch.borrow_mut(),
            count as usize,
            value,
            |bytes| queue.write_buffer(&self.buffers["splatLod"], u64::from(start) * 4, bytes),
        );
    }

    /// Debug colouring of streamed splats (see the scene shader): 0 normal, 1 by level of detail, 2 by distance.
    pub fn set_debug_colour(&self, queue: &wgpu::Queue, mode: u32) {
        queue.write_buffer(&self.buffers["splatDebug"], 0, &mode.to_le_bytes());
    }

    /// Rows of the SH centroid texture.
    pub fn centroid_rows(&self) -> u32 {
        self.textures.get("sh_centroids").map_or(0, |t| t.height())
    }

    /// Writes whole rows of the SH centroid texture (64 centroids × 15 texels per row) from row `first_row`.
    pub fn write_centroid_rows(&self, queue: &wgpu::Queue, first_row: u32, rgba: &[u8]) {
        let texture = &self.textures["sh_centroids"];
        let width = texture.width();
        let rows = (rgba.len() / (width as usize * 4)) as u32;
        if rows == 0 {
            return;
        }
        // Row zero may no longer decode to zero after this public write.
        // LevelStream::init_pool enables the shortcut again after writing its
        // known zero row; uploads to other palette rows leave it unchanged.
        if first_row == 0 {
            self.set_uniform(queue, "sh_zero_reserved", &0u32.to_le_bytes());
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: first_row,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Sorts and draws only the first `active` slots.
    pub fn set_active(&self, queue: &wgpu::Queue, active: u32) {
        let active = active.min(self.count);
        self.sorter.set_active(queue, active);
        // The final direct-draw instance can be partial. Prevent vertices beyond the active prefix from reading
        // stale indices in a streaming pool whose allocated capacity is larger than its active range.
        self.set_uniform(queue, "numSplats", &active.to_le_bytes());
    }

    /// Slot ranges crossfading, sorted by start: (first, one past the last, share of the range drawn). See
    /// `DepthSorter::set_fades`.
    pub fn set_fades(&self, queue: &wgpu::Queue, fades: &[(u32, u32, f32)]) {
        self.sorter.set_fades(queue, fades);
    }

    /// Writes a named uniform of the composed shaders (e.g. `matrix_model`). Unknown names are ignored.
    pub fn set_uniform(&self, queue: &wgpu::Queue, name: &str, bytes: &[u8]) {
        let mut cached = self.uniform_cache.borrow_mut();
        if cached.get(name).is_some_and(|old| old.as_slice() == bytes) {
            self.io_metrics.borrow_mut().count("uniform_cache_hits", 1);
            return;
        }
        {
            let mut m = self.io_metrics.borrow_mut();
            m.count("uniform_writes", 1);
            m.count("uniform_payload_bytes", bytes.len() as u64);
        }
        Buffers(&self.buffers).set(queue, name, bytes);
        if let Some(old) = cached.get_mut(name) {
            old.clear();
            old.extend_from_slice(bytes);
        } else {
            cached.insert(name.to_string(), bytes.to_vec());
        }
    }

    /// Set renderer-neutral camera effects. The vector contains exposure EV,
    /// focus distance in world units, circle-of-confusion scale in pixels, and
    /// shutter fraction of the previous submitted frame. Zero disables each effect.
    pub fn set_camera_effects(&self, queue: &wgpu::Queue, effects: [f32; 4], previous_clip: &Mat4) {
        self.set_uniform(queue, "lens_effects", f32s_le_bytes(&effects).as_ref());
        if effects[3] > 0.0 {
            self.set_uniform(
                queue,
                "previous_clip_from_model",
                f32s_le_bytes(previous_clip).as_ref(),
            );
        }
    }

    pub fn active_slots(&self) -> u32 {
        self.sorter.active_slots()
    }
    pub fn reset_profile(&self) {
        self.io_metrics.borrow_mut().reset();
        self.sorter.reset_profile();
    }
    pub fn profile_json(&self) -> serde_json::Value {
        let streams: BTreeMap<_, _> = self
            .textures
            .iter()
            .map(|(name, texture)| {
                let bytes = u64::from(texture.width())
                    * u64::from(texture.height())
                    * u64::from(texture.format().block_copy_size(None).unwrap_or(0));
                (
                    name,
                    serde_json::json!({"width":texture.width(),"height":texture.height(),
                "format":format!("{:?}",texture.format()),"logical_bytes":bytes}),
                )
            })
            .collect();
        let bytes: u64 = streams
            .values()
            .filter_map(|v| v["logical_bytes"].as_u64())
            .sum();
        serde_json::json!({"io":self.io_metrics.borrow().json(),"sort":self.sorter.profile_json(),
            "stream_textures":{"logical_bytes":bytes,"streams":streams,
                "scope":"SOG stream textures only; excludes buffers, codebooks, frame targets and driver overhead"}})
    }
    /// Slots in this scene.
    pub fn capacity(&self) -> u32 {
        self.count
    }

    /// Writes this frame's matrices and records the depth sort. Call before any pass that draws the scene.
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &View,
        model: &Mat4,
    ) {
        self.prepare_with_cutout(queue, encoder, view, model, None);
    }

    /// `prepare`, also cutting the splats that hide `cutout`'s target (see `sort::Cutout`).
    pub fn prepare_with_cutout(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &View,
        model: &Mat4,
        cutout: Option<&crate::sort::Cutout>,
    ) {
        self.set_uniform(queue, "matrix_model", f32s_le_bytes(model).as_ref());
        self.set_uniform(queue, "matrix_view", f32s_le_bytes(&view.view).as_ref());
        self.set_uniform(
            queue,
            "matrix_projection",
            f32s_le_bytes(&view.projection).as_ref(),
        );
        self.set_uniform(
            queue,
            "camera_params",
            f32s_le_bytes(&[1.0 / view.far, view.far, view.near, 0.0]).as_ref(),
        );
        let (w, h) = (view.width as f32, view.height as f32);
        self.set_uniform(
            queue,
            "viewport_size",
            f32s_le_bytes(&[w, h, 1.0 / w, 1.0 / h]).as_ref(),
        );
        let view_model = camera::mul(&view.view, model);
        let clip = camera::mul(&view.projection, &view_model);
        self.sorter.encode(
            queue,
            encoder,
            &view_model,
            Some(&clip),
            cutout.map(|c| (c, &view.view, model)),
            &self.buffers["splatOrder"],
        );
    }

    /// Draws the sorted splats, blended, into a pass with a colour target and a depth attachment.
    pub fn draw_color(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &Pipelines) {
        self.draw(pass, &pipelines.color.pipeline, &self.color_groups);
    }

    /// Writes the splats' depth into a depth-only pass.
    pub fn draw_depth(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &Pipelines) {
        self.draw(pass, &pipelines.depth.pipeline, &self.depth_groups);
    }

    fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        groups: &[wgpu::BindGroup],
    ) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(pipeline);
        for (index, group) in groups.iter().enumerate() {
            pass.set_bind_group(index as u32, group, &[]);
        }
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);
        // Draw the active prefix directly. The sorter puts visible splats first and the vertex shader culls the
        // remainder. This avoids relying on backend-specific indirect-draw validation while retaining GPU sorting.
        let instances = self.sorter.active_slots().div_ceil(INSTANCE_SIZE);
        pass.draw_indexed(0..INSTANCE_SIZE * 6, 0, 0..instances);
    }

    /// Renders this scene alone offscreen (sky, then splats with `view.model`) and reads it back as tightly packed
    /// RGBA8 rows, top row first. The target is `COLOR_FORMAT` (sRGB), so the bytes are sRGB-encoded.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_rgba(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &Pipelines,
        view: &View,
        sky: Option<(&Sky, f32)>,
    ) -> Vec<u8> {
        let extent = wgpu::Extent3d {
            width: view.width,
            height: view.height,
            depth_or_array_layers: 1,
        };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gsplat color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gsplat depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());

        let bytes_per_row = (view.width * 4).next_multiple_of(256);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gsplat readback"),
            size: u64::from(bytes_per_row) * u64::from(view.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        self.prepare(queue, &mut encoder, view, &view.model);
        {
            let mut pass = color_pass(&mut encoder, &color_view, &depth_view, true, true, "gsplat");
            if let Some((sky, exposure)) = sky {
                sky.draw(queue, &mut pass, &view.view, &view.projection, exposure);
            }
            self.draw_color(&mut pass, pipelines);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(view.height),
                },
            },
            extent,
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll device");
        rx.recv()
            .expect("map callback")
            .expect("map readback buffer");

        let mapped = slice.get_mapped_range().expect("mapped range");
        let row_bytes = (view.width * 4) as usize;
        let mut rgba = Vec::with_capacity(row_bytes * view.height as usize);
        for row in mapped.chunks_exact(bytes_per_row as usize) {
            rgba.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        readback.unmap();
        rgba
    }
}

/// Prepares the exact little-endian per-slot labels. The queue copies the bytes
/// before the callback returns, so the bounded staging buffer can be reused.
fn with_lod_label_bytes(
    scratch: &mut Vec<u32>,
    count: usize,
    value: u32,
    write: impl FnOnce(&[u8]),
) {
    if count > LOD_LABEL_SCRATCH_SPLATS {
        write(&value.to_le_bytes().repeat(count));
        return;
    }
    if scratch.capacity() < count {
        scratch.reserve_exact(count - scratch.len());
    }
    let existing = count.min(scratch.len());
    scratch[..existing].fill(value.to_le());
    scratch.resize(count, value.to_le());
    write(bytemuck::cast_slice(scratch));
}

/// Per-axis 0.5th and 99.5th percentiles: a box that ignores stray splats.
fn robust_bounds(centres: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    if centres.is_empty() {
        return (lo, hi);
    }
    let mut values = vec![0.0f32; centres.len()];
    let (a, b) = (centres.len() / 200, centres.len() - 1 - centres.len() / 200);
    for axis in 0..3 {
        values
            .iter_mut()
            .zip(centres)
            .for_each(|(v, c)| *v = c[axis]);
        lo[axis] = *values.select_nth_unstable_by(a, f32::total_cmp).1;
        hi[axis] = *values.select_nth_unstable_by(b, f32::total_cmp).1;
    }
    (lo, hi)
}

/// Named access to the scene's uniform and storage buffers.
struct Buffers<'a>(&'a BTreeMap<String, wgpu::Buffer>);

impl Buffers<'_> {
    /// Writes `bytes` to the buffer for `name`. Names missing from the binding table are ignored.
    fn set(&self, queue: &wgpu::Queue, name: &str, bytes: &[u8]) {
        if let Some(buffer) = self.0.get(name) {
            queue.write_buffer(buffer, 0, bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_overlapping_slots_preserves_centres_and_all_texture_streams() {
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let pipelines = Pipelines::new(&device, crate::COLOR_FORMAT).unwrap();
        let meta = sog::Meta {
            count: 0,
            means_mins: [-1.0; 3],
            means_maxs: [1.0; 3],
            scales_codebook: vec![0.0; 256],
            sh0_codebook: vec![0.0; 256],
            sh_n_codebook: vec![0.0; 256],
            sh_n_count: 0,
        };
        let pool =
            pool_with_centroid_rows(&device, &queue, &pipelines, &meta, 4096, 0.5, 1).unwrap();
        let count = 300u32;
        let from = 1900u32;
        let first_to = 1770u32;
        let to = 2300u32;
        let centres: Vec<[f32; 3]> = (0..count).map(|i| [i as f32, -(i as f32), 1.0]).collect();
        let streams: Vec<Vec<u8>> = (0..4u32)
            .map(|stream| {
                (0..count)
                    .flat_map(|i| (stream * 10000 + i).to_le_bytes())
                    .collect()
            })
            .collect();
        let names = ["quats", "scales", "sh0", "sh_labels"];
        let inputs: Vec<_> = names
            .iter()
            .zip(&streams)
            .map(|(&name, data)| (name, data.as_slice()))
            .collect();
        pool.write_splats(&queue, from, &inputs, &centres);
        // Queue several overlapping relocations without waiting. The cached
        // buffer cannot be overwritten before the preceding copy has read it.
        pool.move_slots(&queue, from, first_to, count);
        let first_buffer = pool.compaction_buffer(count);
        pool.move_slots(&queue, first_to, 2500, count);
        pool.move_slots(&queue, 2500, to, count);
        assert_eq!(first_buffer, pool.compaction_buffer(count / 2));
        let grown = pool.compaction_buffer(count * 3);
        assert_ne!(first_buffer, grown);
        let retained = grown.size();
        let temporary = pool.compaction_buffer((COMPACTION_SCRATCH_BYTES / 12 + 1) as u32);
        assert!(temporary.size() > COMPACTION_SCRATCH_BYTES);
        assert_eq!(
            pool.compaction_scratch.borrow().as_ref().unwrap().size(),
            retained
        );

        let row_bytes = u64::from(POOL_WIDTH) * 4;
        let texture_bytes = row_bytes * 2;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compaction readback"),
            size: 4096 + 4 * texture_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(
            &pool.centers,
            u64::from(to) * 12,
            &readback,
            0,
            u64::from(count) * 12,
        );
        for (i, name) in names.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &pool.textures[*name],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 4096 + i as u64 * texture_bytes,
                        bytes_per_row: Some(row_bytes as u32),
                        rows_per_image: Some(2),
                    },
                },
                wgpu::Extent3d {
                    width: POOL_WIDTH,
                    height: 2,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range().unwrap();
        assert_eq!(
            &bytes[..count as usize * 12],
            centres_le_bytes(&centres).as_ref()
        );
        for (i, expected) in streams.iter().enumerate() {
            let start = 4096 + i * texture_bytes as usize + to as usize * 4;
            assert_eq!(
                &bytes[start..start + expected.len()],
                expected.as_slice(),
                "{}",
                names[i]
            );
        }
    }

    #[test]
    #[ignore = "manual GPU resource-allocation benchmark"]
    fn benchmark_compaction_buffer_reuse() {
        use std::{hint::black_box, time::Instant};
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let pipelines = Pipelines::new(&device, crate::COLOR_FORMAT).unwrap();
        let meta = sog::Meta {
            count: 0,
            means_mins: [-1.0; 3],
            means_maxs: [1.0; 3],
            scales_codebook: vec![0.0; 256],
            sh0_codebook: vec![0.0; 256],
            sh_n_codebook: vec![0.0; 256],
            sh_n_count: 0,
        };
        let pool =
            pool_with_centroid_rows(&device, &queue, &pipelines, &meta, 4096, 0.5, 1).unwrap();
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..9 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                for i in 0..400 {
                    let count = if i % 2 == 0 { 8192 } else { 16384 };
                    if mode == 0 {
                        black_box(device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("fresh compaction scratch"),
                            size: u64::from(count) * 12,
                            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }));
                    } else {
                        black_box(pool.compaction_buffer(count));
                    }
                }
                samples[mode].push(start.elapsed().as_nanos() as f64 / 400.0);
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "compaction buffer: fresh {:.0} ns, reused {:.0} ns",
            samples[0][4], samples[1][4]
        );
    }

    #[test]
    fn released_centres_are_nan_without_touching_other_slots() {
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let pipelines = Pipelines::new(&device, crate::COLOR_FORMAT).unwrap();
        let meta = sog::Meta {
            count: 0,
            means_mins: [-1.0; 3],
            means_maxs: [1.0; 3],
            scales_codebook: vec![0.0; 256],
            sh0_codebook: vec![0.0; 256],
            sh_n_codebook: vec![0.0; 256],
            sh_n_count: 0,
        };
        let capacity = RELEASE_SCRATCH_SPLATS as u32 + 16;
        let pool =
            pool_with_centroid_rows(&device, &queue, &pipelines, &meta, capacity, 0.5, 1).unwrap();
        let centres: Vec<_> = (0..16).map(|i| [i as f32, 2.0, -1.0]).collect();
        pool.write_splats(&queue, 0, &[], &centres);
        pool.release(&queue, 1, 3);
        let scratch_capacity = pool.release_scratch.borrow().capacity();
        pool.release(&queue, 5, 2);
        assert_eq!(pool.release_scratch.borrow().capacity(), scratch_capacity);
        // Exercise the temporary large-release path without growing retained scratch.
        pool.release(&queue, 8, RELEASE_SCRATCH_SPLATS as u32 + 1);
        assert_eq!(pool.release_scratch.borrow().capacity(), scratch_capacity);

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("retired centre readback"),
            size: 16 * 12,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&pool.centers, 0, &readback, 0, 16 * 12);
        queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range().unwrap();
        for (index, xyz) in bytes.chunks_exact(12).enumerate() {
            let retired = (1..4).contains(&index) || (5..7).contains(&index) || index >= 8;
            for (axis, value) in xyz.chunks_exact(4).enumerate() {
                let bits = u32::from_le_bytes(value.try_into().unwrap());
                let expected = if retired {
                    f32::NAN
                } else {
                    centres[index][axis]
                };
                assert_eq!(bits, expected.to_bits(), "slot {index} axis {axis}");
            }
        }
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_retired_centre_preparation() {
        use std::{hint::black_box, time::Instant};
        let mut scratch = Vec::<[f32; 3]>::new();
        let mut old_samples = Vec::new();
        let mut new_samples = Vec::new();
        for sample in 0..9 {
            for old in if sample % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let start = Instant::now();
                for i in 0..400 {
                    let count = black_box(if i % 2 == 0 { 16_384 } else { 32_768 });
                    if old {
                        let empty = vec![f32::NAN; count * 3];
                        black_box(f32s(&empty));
                    } else {
                        scratch.resize(count, [f32::NAN; 3]);
                        black_box(centres_le_bytes(&scratch));
                    }
                }
                let ns = start.elapsed().as_nanos() as f64 / 400.0;
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
            "retired centres: old {:.0} ns, reused {:.0} ns",
            old_samples[4], new_samples[4]
        );
    }

    #[test]
    fn lod_label_staging_preserves_bytes_and_bounds_retained_memory() {
        let mut scratch = Vec::new();
        for (count, value) in [
            (4096, 0x12345678u32),
            (17, 0xfedcba98),
            (8192, 0),
            (1, u32::MAX),
            (0, 42),
            (LOD_LABEL_SCRATCH_SPLATS, 0x87654321),
        ] {
            with_lod_label_bytes(&mut scratch, count, value, |bytes| {
                assert_eq!(bytes, value.to_le_bytes().repeat(count));
            });
        }
        let capacity = scratch.capacity();
        with_lod_label_bytes(
            &mut scratch,
            LOD_LABEL_SCRATCH_SPLATS + 1,
            0x13572468,
            |bytes| {
                assert_eq!(
                    bytes,
                    0x13572468u32
                        .to_le_bytes()
                        .repeat(LOD_LABEL_SCRATCH_SPLATS + 1)
                )
            },
        );
        assert_eq!(scratch.capacity(), capacity);
    }

    #[test]
    fn lod_label_staging_uploads_exact_values_at_the_requested_offset() {
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LOD label test target"),
            size: 64,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LOD label readback"),
            size: 64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut scratch = Vec::new();
        with_lod_label_bytes(&mut scratch, 12, 0x12345678, |bytes| {
            queue.write_buffer(&buffer, 0, bytes)
        });
        with_lod_label_bytes(&mut scratch, 3, 0xaabbccdd, |bytes| {
            queue.write_buffer(&buffer, 16, bytes)
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&buffer, 0, &readback, 0, 64);
        queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range().unwrap();
        for (index, word) in bytes.chunks_exact(4).enumerate() {
            let expected: u32 = if (4..7).contains(&index) {
                0xaabbccdd
            } else if index < 12 {
                0x12345678
            } else {
                0
            };
            assert_eq!(word, expected.to_le_bytes(), "slot {index}");
        }
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_lod_label_staging() {
        use std::{hint::black_box, time::Instant};
        for count in [512usize, 2_048, 8_192, 32_768] {
            let mut scratch = Vec::new();
            let mut samples = [Vec::new(), Vec::new()];
            for round in 0..9 {
                for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let start = Instant::now();
                    for i in 0..4000u32 {
                        let value = black_box(i.wrapping_mul(17));
                        if mode == 0 {
                            black_box(value.to_le_bytes().repeat(count));
                        } else {
                            with_lod_label_bytes(&mut scratch, count, value, |bytes| {
                                black_box(bytes);
                            });
                        }
                    }
                    samples[mode].push(start.elapsed().as_nanos() / 4000);
                }
            }
            samples[0].sort();
            samples[1].sort();
            println!(
                "count={count} fresh_ns={} reused_ns={}",
                samples[0][4], samples[1][4]
            );
        }
    }

    #[test]
    fn uniform_float_bytes_preserve_all_bits_and_endianness() {
        let values = [
            0.0,
            -0.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::from_bits(0x7fc0_1234),
            f32::from_bits(1),
            f32::MIN,
            f32::MAX,
        ];
        let encoded = f32s_le_bytes(&values);
        assert_eq!(encoded.as_ref(), f32s(&values));
        assert!(f32s_le_bytes(&[]).is_empty());
        #[cfg(target_endian = "little")]
        assert!(matches!(encoded, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_uniform_float_bytes() {
        use std::{hint::black_box, time::Instant};
        let matrix = crate::camera::IDENTITY;
        let cached = f32s(&matrix);
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..11 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                for _ in 0..500_000 {
                    let equal = if mode == 0 {
                        f32s(black_box(&matrix)).as_slice() == black_box(&cached)
                    } else {
                        f32s_le_bytes(black_box(&matrix)).as_ref() == black_box(&cached)
                    };
                    assert!(black_box(equal));
                }
                samples[mode].push(start.elapsed().as_nanos() as f64 / 500_000.0);
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "matrix uniform bytes: allocated {:.1} ns, borrowed {:.1} ns",
            samples[0][5], samples[1][5]
        );
    }

    #[test]
    fn centre_bytes_preserve_float_bits_and_little_endian_order() {
        let centres = [
            [0.0, -0.0, f32::INFINITY],
            [
                f32::NEG_INFINITY,
                f32::from_bits(0x7fc0_1234),
                f32::from_bits(0xffc0_4321),
            ],
            [
                f32::from_bits(0x7fa0_0001),
                f32::from_bits(0xffa0_0001),
                f32::from_bits(1),
            ],
            [f32::MIN, f32::MAX, -37.25],
        ];
        let original = f32s(&centres.concat());
        let bytes = centres_le_bytes(&centres);
        assert_eq!(bytes.as_ref(), original.as_slice());
        assert_eq!(centres_to_le_bytes(&centres), original);
        assert!(centres_le_bytes(&[]).is_empty());
        #[cfg(target_endian = "little")]
        assert!(matches!(bytes, std::borrow::Cow::Borrowed(_)));
    }
}
