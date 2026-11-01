//! wgpu side of the gsplat port.
//!
//! `countersplat-shaders` produces composed, validated WGSL plus a binding table. This crate turns that
//! table into wgpu bind-group layouts and creates the shader modules.

pub mod camera;
pub mod env;
pub mod metrics;
pub mod presentation;
pub mod scene;
pub mod sort;
pub mod stream;
pub mod voxel_debug;
pub mod worker_decode;

// Re-export asset modules for downstream code that already works with render scenes.
pub use countersplat_assets::{lod, sog};

use countersplat_shaders::binding::{Binding, BindingKind};
use std::collections::BTreeMap;

/// Bind-group layout entries grouped by `@group` index, built from a binding table.
///
/// Uniforms are shared across stages except the vertex-only SH capability. Textures are `Float` sampled textures;
/// the engine's unfilterable sentinel (RGBA32F codebook) is `filterable: false`. Samplers are
/// `NonFiltering`, because the engine samples splat data with `textureLoad`, not filtered sampling.
/// Storage buffers serve vertex animation and fragment opacity cleanup, so both stages can read them.
pub fn bind_group_entries(bindings: &[Binding]) -> BTreeMap<u32, Vec<wgpu::BindGroupLayoutEntry>> {
    let mut groups: BTreeMap<u32, Vec<wgpu::BindGroupLayoutEntry>> = BTreeMap::new();
    for b in bindings {
        let entry = match &b.kind {
            BindingKind::Uniform { .. } => wgpu::BindGroupLayoutEntry {
                binding: b.binding,
                // Keep the portable 12-uniform per-stage budget: these
                // uniforms are consumed by one stage each.
                visibility: if b.name == "sh_zero_reserved" {
                    wgpu::ShaderStages::VERTEX
                } else {
                    wgpu::ShaderStages::VERTEX_FRAGMENT
                },
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindingKind::Texture { unfilterable, .. } => wgpu::BindGroupLayoutEntry {
                binding: b.binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float {
                        filterable: !unfilterable,
                    },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindingKind::Sampler { .. } => wgpu::BindGroupLayoutEntry {
                binding: b.binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            },
            BindingKind::Storage { access, .. } => wgpu::BindGroupLayoutEntry {
                binding: b.binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: access == "read",
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        };
        groups.entry(b.group).or_default().push(entry);
    }
    groups
}

/// Creates a wgpu shader module from composed WGSL.
pub fn shader_module(device: &wgpu::Device, label: &str, wgsl: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    })
}

/// Creates one bind-group layout per group in the table, in group order.
pub fn bind_group_layouts(
    device: &wgpu::Device,
    bindings: &[Binding],
) -> Vec<wgpu::BindGroupLayout> {
    bind_group_entries(bindings)
        .into_iter()
        .map(|(group, entries)| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(&format!("gsplat group {group}")),
                entries: &entries,
            })
        })
        .collect()
}

/// Requests a device on any available adapter, or `None` when no GPU is reachable (e.g. CI).
#[cfg(not(target_arch = "wasm32"))]
pub fn try_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Format of the scene colour target. The engine's default is RGBA8 with premultiplied alpha.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Depth format. The colour passes test against it without writing; the depth prepass writes it (see `scene`).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Creates the gsplat render pipeline from composed vertex and fragment modules.
///
/// - Vertex input: one `vec3f` attribute at `@location(0)` (`vertex_position`), stepped per vertex.
///   Splat quads come from the vertex buffer; the splat index comes from `instance_index`.
/// - Blend: premultiplied "over", matching the engine's fragment output.
/// - Depth: tested, not written.
pub fn gsplat_pipeline(
    device: &wgpu::Device,
    vertex: &wgpu::ShaderModule,
    fragment: &wgpu::ShaderModule,
    layouts: &[wgpu::BindGroupLayout],
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let refs: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gsplat pipeline layout"),
        bind_group_layouts: &refs,
        immediate_size: 0,
    });
    let attributes = [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    }];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gsplat"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: vertex,
            entry_point: Some("vertexMain"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: fragment,
            entry_point: Some("fragmentMain"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// The depth prepass pipeline, from the `PREPASS_PASS` modules: no colour, depth written and tested. The fragment
/// stage discards below `alphaClip`, so only the solid core of each splat claims depth.
pub fn gsplat_depth_pipeline(
    device: &wgpu::Device,
    vertex: &wgpu::ShaderModule,
    fragment: &wgpu::ShaderModule,
    layouts: &[wgpu::BindGroupLayout],
) -> wgpu::RenderPipeline {
    let refs: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gsplat depth layout"),
        bind_group_layouts: &refs,
        immediate_size: 0,
    });
    let attributes = [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    }];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gsplat depth"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: vertex,
            entry_point: Some("vertexMain"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: fragment,
            entry_point: Some("fragmentMain"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
