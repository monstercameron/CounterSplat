//! A small GPU line renderer for host-provided world-space snapshots.
//! It knows nothing about ECS, collision, weapons, or target rules.
use crate::camera::{self, Mat4};

pub const MAX_TRACE_SEGMENTS: usize = 1024;

pub struct TracerRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    vertex_count: u32,
}
impl TracerRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("projectile trace camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("projectile trace vertices"),
            size: (MAX_TRACE_SEGMENTS * 2 * 4 * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("projectile trace bindings"),
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
            label: Some("projectile trace bindings"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("projectile trace shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/tracer.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("projectile trace pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = [wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 0,
            shader_location: 0,
        }];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("projectile trace pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
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
                topology: wgpu::PrimitiveTopology::LineList,
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
            uniform,
            vertices,
            vertex_count: 0,
        }
    }
    /// Replace the snapshot with packed world-space start XYZ, end XYZ, alpha.
    pub fn set(&mut self, queue: &wgpu::Queue, segments: &[f32]) -> Result<(), &'static str> {
        if !segments.len().is_multiple_of(7)
            || segments.len() / 7 > MAX_TRACE_SEGMENTS
            || !segments
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
            || segments
                .chunks_exact(7)
                .any(|v| !(0.0..=1.0).contains(&v[6]))
        {
            return Err("invalid world-space line snapshot");
        }
        let mut vertices = Vec::with_capacity(segments.len() / 7 * 8);
        for s in segments.chunks_exact(7) {
            vertices.extend_from_slice(&[s[0], s[1], s[2], s[6], s[3], s[4], s[5], s[6]]);
        }
        if !vertices.is_empty() {
            queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));
        }
        self.vertex_count = (vertices.len() / 4) as u32;
        Ok(())
    }
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: &Mat4,
        projection: &Mat4,
    ) {
        if self.vertex_count == 0 {
            return;
        }
        let matrix = camera::mul(projection, view);
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&matrix));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}
