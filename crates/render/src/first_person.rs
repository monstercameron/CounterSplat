//! Backend-neutral compositing of keyed first-person image frames into the HDR scene.
//! Gameplay and animation selection are intentionally owned by the caller.
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub visible: bool,
    pub opacity: f32,
    pub key_threshold: f32,
    pub key_softness: f32,
    pub despill: f32,
    pub vertical_offset: f32,
    /// Script-authored screen-space movement and roll, in normalized frame units/radians.
    pub motion_x: f32,
    pub motion_y: f32,
    pub motion_roll: f32,
    /// Blend from the captured previous frame to the current frame (0..1).
    pub blend: f32,
    /// Half the directional blur span in source-frame UV coordinates.
    pub blur_x: f32,
    pub blur_y: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            visible: false,
            opacity: 1.0,
            key_threshold: 0.14,
            key_softness: 0.28,
            despill: 0.025,
            vertical_offset: 0.055,
            motion_x: 0.0,
            motion_y: 0.0,
            motion_roll: 0.0,
            blend: 1.0,
            blur_x: 0.0,
            blur_y: 0.0,
        }
    }
}
fn finite(v: f32, default: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

pub struct FirstPersonLayer {
    layout: wgpu::BindGroupLayout,
    bind: wgpu::BindGroup,
    texture: wgpu::Texture,
    previous: wgpu::Texture,
    previous_ready: bool,
    uniform: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    size: (u32, u32),
    ready: bool,
}
impl FirstPersonLayer {
    fn texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("first-person streamed frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }
    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        texture: &wgpu::Texture,
        previous: &wgpu::Texture,
        uniform: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("first-person frame bindings"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &previous.create_view(&Default::default()),
                    ),
                },
            ],
        })
    }
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("first-person frame layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("first-person key settings"),
            contents: bytemuck::cast_slice(&[0.0f32; 16]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let texture = Self::texture(device, 1, 1);
        let previous = Self::texture(device, 1, 1);
        let bind = Self::bind(device, &layout, &texture, &previous, &uniform);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("first-person key shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/first_person.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("first-person pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("first-person compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            layout,
            bind,
            texture,
            previous,
            previous_ready: false,
            uniform,
            pipeline,
            size: (1, 1),
            ready: false,
        }
    }
    /// Reallocate only when the streamed image dimensions change. The host uploads
    /// pixels into the returned texture and then calls `mark_ready`.
    pub fn frame_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> &wgpu::Texture {
        if self.size != (width, height) {
            self.texture = Self::texture(device, width, height);
            self.previous = Self::texture(device, width, height);
            self.bind = Self::bind(
                device,
                &self.layout,
                &self.texture,
                &self.previous,
                &self.uniform,
            );
            self.size = (width, height);
            self.ready = false;
            self.previous_ready = false;
        }
        &self.texture
    }
    pub fn mark_ready(&mut self) {
        self.ready = true;
    }
    /// Capture the displayed source frame for a presentation-only clip handoff.
    /// The queue orders this copy before the host uploads the next decoded frame.
    pub fn capture_previous(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.ready {
            return false;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("capture first-person transition frame"),
        });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.previous,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.size.0,
                height: self.size.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        self.previous_ready = true;
        true
    }
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        size: (u32, u32),
        settings: Settings,
    ) {
        if !self.ready || !settings.visible {
            return;
        }
        let values = [
            finite(settings.key_threshold, 0.14, 0., 0.6),
            finite(settings.key_softness, 0.28, 0.01, 0.8),
            finite(settings.despill, 0.025, 0., 0.2),
            finite(settings.opacity, 1., 0., 1.),
            finite(settings.vertical_offset, 0.055, -0.25, 0.25),
            size.0.max(1) as f32,
            size.1.max(1) as f32,
            self.size.0 as f32 / self.size.1.max(1) as f32,
            finite(settings.motion_x, 0.0, -0.2, 0.2),
            finite(settings.motion_y, 0.0, -0.5, 0.5),
            finite(settings.motion_roll, 0.0, -0.2, 0.2),
            if self.previous_ready {
                finite(settings.blend, 1.0, 0.0, 1.0)
            } else {
                1.0
            },
            finite(settings.blur_x, 0.0, -0.04, 0.04),
            finite(settings.blur_y, 0.0, -0.04, 0.04),
            0.0,
            0.0,
        ];
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&values));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("first-person footage"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_is_valid_wgsl() {
        let module =
            naga::front::wgsl::parse_str(include_str!("shaders/first_person.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
