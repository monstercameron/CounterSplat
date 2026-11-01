//! Bounded, instanced wireframe visualization for collision voxel centers.
use crate::camera::{self, Mat4};

pub const MAX_DEBUG_VOXELS: usize = 10_000;

pub struct VoxelDebug {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    count: u32,
    size: f32,
}
impl VoxelDebug {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel debug camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel debug instances"),
            size: (MAX_DEBUG_VOXELS * 3 * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxel debug bindings"),
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
            label: Some("voxel debug bindings"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel debug"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/voxel_debug.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel debug"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = [wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel debug"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
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
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
            instances,
            count: 0,
            size: 0.0,
        }
    }
    /// Replace the debug snapshot. Positions are world-space centers in triples.
    pub fn set(
        &mut self,
        queue: &wgpu::Queue,
        centers: &[f32],
        size: f32,
    ) -> Result<(), &'static str> {
        if !centers.len().is_multiple_of(3)
            || centers.len() / 3 > MAX_DEBUG_VOXELS
            || !centers.iter().all(|v| v.is_finite())
            || !size.is_finite()
            || size <= 0.0
            || size > 1000.0
        {
            return Err("invalid voxel debug snapshot");
        }
        if !centers.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(centers));
        }
        self.count = (centers.len() / 3) as u32;
        self.size = size;
        Ok(())
    }
    pub fn clear(&mut self) {
        self.count = 0;
    }
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
        let matrix = camera::mul(projection, view);
        let mut data = [0.0f32; 20];
        data[..16].copy_from_slice(&matrix);
        data[16] = self.size;
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&data));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..24, 0..self.count);
    }
}
