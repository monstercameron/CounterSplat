//! Instanced world-space impact marks and smoke, supplied as render snapshots.
//! Gameplay, collision, lifetime and ECS ownership remain outside this crate.
use crate::camera::{self, Mat4};

/// A bounded snapshot avoids unbounded GPU buffers from script-provided effects.
pub const MAX_IMPACT_SPRITES: usize = 256;
const FLOATS_PER_SPRITE: usize = 12;

pub struct ImpactRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    sprites: wgpu::Buffer,
    count: u32,
}
impl ImpactRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("impact camera"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sprites = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("impact sprites"),
            size: (MAX_IMPACT_SPRITES * FLOATS_PER_SPRITE * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("impact camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("impact camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("impact decals and smoke"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/impact.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("impact pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 32,
                shader_location: 2,
            },
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("impact decals and smoke"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 48,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            bind_group,
            camera,
            sprites,
            count: 0,
        }
    }
    /// Packed instances: center XYZ/radius, surface normal XYZ/kind (0 mark,
    /// 1 smoke), opacity/variation/reserved/reserved. Replaces the snapshot.
    pub fn set(&mut self, queue: &wgpu::Queue, sprites: &[f32]) -> Result<(), &'static str> {
        if !sprites.len().is_multiple_of(FLOATS_PER_SPRITE)
            || sprites.len() / FLOATS_PER_SPRITE > MAX_IMPACT_SPRITES
            || !sprites
                .iter()
                .all(|n| n.is_finite() && n.abs() <= 10_000_000.0)
            || sprites.chunks_exact(FLOATS_PER_SPRITE).any(|s| {
                !(0.0001..=1000.0).contains(&s[3])
                    || (s[7] != 0.0 && s[7] != 1.0)
                    || !(0.0..=1.0).contains(&s[8])
                    || s[4] * s[4] + s[5] * s[5] + s[6] * s[6] < 1e-10
            })
        {
            return Err("invalid impact sprite snapshot");
        }
        if !sprites.is_empty() {
            queue.write_buffer(&self.sprites, 0, bytemuck::cast_slice(sprites));
        }
        self.count = (sprites.len() / FLOATS_PER_SPRITE) as u32;
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    /// Draw after a scene depth pass, with surface marks before the smoke instances.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: &Mat4,
        projection: &Mat4,
    ) {
        if self.count == 0 {
            return;
        }
        let mut uniform = [0.0f32; 24];
        uniform[..16].copy_from_slice(&camera::mul(projection, view));
        uniform[16..19].copy_from_slice(&[view[0], view[4], view[8]]);
        uniform[20..23].copy_from_slice(&[view[1], view[5], view[9]]);
        queue.write_buffer(&self.camera, 0, bytemuck::cast_slice(&uniform));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.sprites.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}
