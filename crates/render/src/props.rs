//! Solid, lit props: boxes and upright cylinders drawn as instanced meshes. They write depth, so drawn before the
//! splats they hide the scan behind them and the scan in front of them hides them. Knows nothing about physics:
//! the host passes each prop's pose, scale and colours.
use crate::camera::{self, Mat4};

pub const MAX_PROP_INSTANCES: usize = 256;
const CYLINDER_SEGMENTS: usize = 24;
/// Floats per instance: position 3, rotation 4, scale 3, colour 3, cap colour 3.
const INSTANCE_FLOATS: usize = 16;

/// One prop to draw. `scale` is a box's half extents, or a cylinder's radius, half height and radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropInstance {
    pub cylinder: bool,
    pub position: [f32; 3],
    /// Quaternion x, y, z, w.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Linear RGB.
    pub color: [f32; 3],
    pub cap_color: [f32; 3],
}

struct Mesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    instances: wgpu::Buffer,
    instance_count: u32,
}

pub struct PropRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    cube: Mesh,
    cylinder: Mesh,
}

/// Vertices (position, normal, cap flag) and triangle indices of a cube spanning -1..1.
fn cube_mesh() -> (Vec<f32>, Vec<u16>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut normal = [0.0; 3];
            normal[axis] = sign;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let base = (vertices.len() / 7) as u16;
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let mut p = [0.0f32; 3];
                p[axis] = sign;
                p[u] = a;
                p[v] = b;
                vertices.extend_from_slice(&[p[0], p[1], p[2], normal[0], normal[1], normal[2], 0.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    (vertices, indices)
}

/// Vertices and indices of a cylinder of radius 1 from y = -1 to 1: a smooth side and two flat caps (cap flag 1).
fn cylinder_mesh() -> (Vec<f32>, Vec<u16>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let n = CYLINDER_SEGMENTS;
    let ring = |i: usize| {
        let angle = i as f32 / n as f32 * std::f32::consts::TAU;
        (angle.cos(), angle.sin())
    };
    for i in 0..=n {
        let (c, s) = ring(i);
        vertices.extend_from_slice(&[c, -1.0, s, c, 0.0, s, 0.0]);
        vertices.extend_from_slice(&[c, 1.0, s, c, 0.0, s, 0.0]);
    }
    for i in 0..n as u16 {
        let (a, b) = (i * 2, i * 2 + 2);
        indices.extend_from_slice(&[a, a + 1, b + 1, a, b + 1, b]);
    }
    for (y, sign) in [(-1.0f32, -1.0f32), (1.0, 1.0)] {
        let centre = (vertices.len() / 7) as u16;
        vertices.extend_from_slice(&[0.0, y, 0.0, 0.0, sign, 0.0, 1.0]);
        for i in 0..=n {
            let (c, s) = ring(i);
            vertices.extend_from_slice(&[c, y, s, 0.0, sign, 0.0, 1.0]);
        }
        for i in 0..n as u16 {
            indices.extend_from_slice(&[centre, centre + 1 + i, centre + 2 + i]);
        }
    }
    (vertices, indices)
}

impl PropRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prop camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prop bindings"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prop bindings"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prop shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/props.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prop pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32];
        let instance_attributes = wgpu::vertex_attr_array![
            3 => Float32x3, 4 => Float32x4, 5 => Float32x3, 6 => Float32x3, 7 => Float32x3
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prop pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 7 * 4,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attributes,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: (INSTANCE_FLOATS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            // Both windings are drawn (the meshes are closed, and there are only a few props).
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let mesh = |label: &str, (vertices, indices): (Vec<f32>, Vec<u16>)| {
            use wgpu::util::DeviceExt;
            Mesh {
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                index_count: indices.len() as u32,
                instances: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: (MAX_PROP_INSTANCES * INSTANCE_FLOATS * 4) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                instance_count: 0,
            }
        };
        Self {
            pipeline,
            bind_group,
            uniform,
            cube: mesh("prop boxes", cube_mesh()),
            cylinder: mesh("prop cylinders", cylinder_mesh()),
        }
    }

    /// Replaces the props to draw.
    pub fn set(&mut self, queue: &wgpu::Queue, props: &[PropInstance]) -> Result<(), &'static str> {
        if props.len() > MAX_PROP_INSTANCES {
            return Err("too many props to draw");
        }
        let valid = |p: &PropInstance| {
            p.position.iter().all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
                && p.rotation.iter().all(|v| v.is_finite())
                && p.scale.iter().all(|v| v.is_finite() && *v > 0.0)
                && p.color.iter().chain(&p.cap_color).all(|v| (0.0..=8.0).contains(v))
        };
        if !props.iter().all(valid) {
            return Err("invalid prop instance");
        }
        for (mesh, cylinder) in [(&mut self.cube, false), (&mut self.cylinder, true)] {
            let data: Vec<f32> = props
                .iter()
                .filter(|p| p.cylinder == cylinder)
                .flat_map(|p| {
                    let mut row = [0.0f32; INSTANCE_FLOATS];
                    row[0..3].copy_from_slice(&p.position);
                    row[3..7].copy_from_slice(&p.rotation);
                    row[7..10].copy_from_slice(&p.scale);
                    row[10..13].copy_from_slice(&p.color);
                    row[13..16].copy_from_slice(&p.cap_color);
                    row
                })
                .collect();
            if !data.is_empty() {
                queue.write_buffer(&mesh.instances, 0, bytemuck::cast_slice(&data));
            }
            mesh.instance_count = (data.len() / INSTANCE_FLOATS) as u32;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.cube.instance_count == 0 && self.cylinder.instance_count == 0
    }

    /// Draws the props into a pass with a depth attachment, before the splats. `exposure` scales their colour like
    /// the camera's exposure scales the scan.
    pub fn draw(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, view: &Mat4, projection: &Mat4, exposure: f32) {
        if self.is_empty() {
            return;
        }
        let matrix = camera::mul(projection, view);
        let mut uniform = [0.0f32; 20];
        uniform[..16].copy_from_slice(&matrix);
        // Light from high up and to one side.
        uniform[16..20].copy_from_slice(&[0.35, 0.85, 0.4, if exposure.is_finite() { exposure.max(0.0) } else { 1.0 }]);
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&uniform));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        for mesh in [&self.cube, &self.cylinder] {
            if mesh.instance_count == 0 {
                continue;
            }
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_vertex_buffer(1, mesh.instances.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..mesh.index_count, 0, 0..mesh.instance_count);
        }
    }
}
