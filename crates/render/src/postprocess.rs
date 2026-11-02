//! GPU-only post-processing. Hosts render linear scene color into `scene_view`, then
//! call `encode` before drawing UI. The half-resolution bloom stages and final
//! correction are separate passes so more effects can be inserted between them.
use wgpu::util::DeviceExt;

/// Portable, blendable linear HDR scene format.
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// How an independently sized scene is mapped to an output viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportScale {
    /// Fill the viewport and crop equally at the edges when aspect ratios differ.
    Fill,
    /// Show the complete scene with black bars when aspect ratios differ.
    Fit,
    /// Replicate whole source pixels by an integer factor. If the source is
    /// larger than the output, present at 1x and crop instead of resampling.
    PixelPerfect,
}

/// UV scale for aspect-preserving presentation. Input and output sizes must be nonzero.
pub fn viewport_uv_scale(source: (u32, u32), output: (u32, u32), mode: ViewportScale) -> [f32; 2] {
    let source_aspect = source.0.max(1) as f32 / source.1.max(1) as f32;
    let output_aspect = output.0.max(1) as f32 / output.1.max(1) as f32;
    let ratio = output_aspect / source_aspect;
    match mode {
        ViewportScale::Fill if ratio < 1.0 => [ratio, 1.0],
        ViewportScale::Fill => [1.0, 1.0 / ratio],
        ViewportScale::Fit if ratio < 1.0 => [1.0, 1.0 / ratio],
        ViewportScale::Fit => [ratio, 1.0],
        ViewportScale::PixelPerfect => {
            let source = (source.0.max(1), source.1.max(1));
            let output = (output.0.max(1), output.1.max(1));
            let factor = (output.0 / source.0).min(output.1 / source.1).max(1);
            [
                output.0 as f32 / (source.0 as f32 * factor as f32),
                output.1 as f32 / (source.1 as f32 * factor as f32),
            ]
        }
    }
}

/// Integer-aligned centering correction, including odd-sized bars and crops.
pub fn viewport_uv_offset(source: (u32, u32), output: (u32, u32), mode: ViewportScale) -> [f32; 2] {
    if mode != ViewportScale::PixelPerfect {
        return [0.0, 0.0];
    }
    let source = (source.0.max(1), source.1.max(1));
    let output = (output.0.max(1), output.1.max(1));
    let factor = (output.0 / source.0).min(output.1 / source.1).max(1);
    let correction = |src: u32, out: u32| {
        let image = i64::from(src) * i64::from(factor);
        let difference = i64::from(out) - image;
        (difference as f64 / 2.0 - difference.div_euclid(2) as f64) as f32 / image as f32
    };
    [
        correction(source.0, output.0),
        correction(source.1, output.1),
    ]
}

/// Renderer-facing values, independent of ECS and scripting. Exposure is in stops.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub enabled: bool,
    pub filmic: bool,
    pub exposure_ev: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub shadow_warmth: f32,
    pub highlight_warmth: f32,
    pub shadows: f32,
    pub highlights: f32,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub vignette: f32,
    pub grain: f32,
    pub sharpness: f32,
    /// Transient speed-driven edge blur. Zero skips the extra samples.
    pub peripheral_blur: f32,
    /// Transient firing-driven tunnel effect; kept separate so both can blend.
    pub tunnel_vision: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            filmic: false,
            exposure_ev: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            shadow_warmth: 0.0,
            highlight_warmth: 0.0,
            shadows: 0.0,
            highlights: 0.0,
            bloom_intensity: 0.0,
            bloom_threshold: 1.0,
            vignette: 0.0,
            grain: 0.0,
            sharpness: 0.0,
            peripheral_blur: 0.0,
            tunnel_vision: 0.0,
        }
    }
}
impl Settings {
    fn uniform(self) -> [f32; 24] {
        // Keep the low-level interface safe even when a host does not validate its input.
        let finite = |v: f32, fallback: f32, lo: f32, hi: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                fallback
            }
        };
        [
            finite(self.exposure_ev, 0.0, -8.0, 8.0),
            finite(self.contrast, 1.0, 0.25, 2.0),
            finite(self.saturation, 1.0, 0.0, 2.0),
            finite(self.temperature, 0.0, -1.0, 1.0),
            finite(self.tint, 0.0, -1.0, 1.0),
            finite(self.shadows, 0.0, -1.0, 1.0),
            finite(self.highlights, 0.0, -1.0, 1.0),
            finite(self.vignette, 0.0, 0.0, 1.0),
            finite(self.bloom_intensity, 0.0, 0.0, 2.0),
            finite(self.bloom_threshold, 1.0, 0.01, 16.0),
            finite(self.grain, 0.0, 0.0, 0.2),
            self.enabled as u8 as f32,
            self.filmic as u8 as f32,
            1.0,
            1.0,
            finite(self.sharpness, 0.0, 0.0, 2.0),
            finite(self.shadow_warmth, 0.0, -1.0, 1.0),
            finite(self.highlight_warmth, 0.0, -1.0, 1.0),
            finite(self.peripheral_blur, 0.0, 0.0, 1.0),
            finite(self.tunnel_vision, 0.0, 0.0, 1.0),
            0.0,
            0.0,
            0.0,
            0.0,
        ]
    }
}

struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}
impl Target {
    fn new(device: &wgpu::Device, label: &str, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            _texture: texture,
            view,
        }
    }
}
struct Targets {
    scene: Target,
    _ping: Target,
    _pong: Target,
    extract: wgpu::BindGroup,
    horizontal: wgpu::BindGroup,
    vertical: wgpu::BindGroup,
    composite: wgpu::BindGroup,
}

/// Owns reusable targets and pipelines. Recreate its targets on canvas resize.
pub struct PostProcess {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    extract: wgpu::RenderPipeline,
    horizontal: wgpu::RenderPipeline,
    vertical: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    targets: Targets,
    width: u32,
    height: u32,
}
impl PostProcess {
    pub fn new(
        device: &wgpu::Device,
        output_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post-process inputs"),
            entries: &[
                texture_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(3),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post-process linear sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("image correction settings"),
            contents: bytemuck::cast_slice(&Settings::default().uniform()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post-process shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/postprocess.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post-process pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let extract = pipeline("prefilter", SCENE_FORMAT);
        let horizontal = pipeline("blur_horizontal", SCENE_FORMAT);
        let vertical = pipeline("blur_vertical", SCENE_FORMAT);
        let composite = pipeline("correct", output_format);
        let targets = Self::targets(device, &layout, &sampler, &uniform, width, height);
        Self {
            layout,
            sampler,
            uniform,
            extract,
            horizontal,
            vertical,
            composite,
            targets,
            width: width.max(1),
            height: height.max(1),
        }
    }
    fn targets(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        uniform: &wgpu::Buffer,
        width: u32,
        height: u32,
    ) -> Targets {
        let scene = Target::new(device, "HDR scene color", width, height);
        let ping = Target::new(
            device,
            "bloom ping",
            width.max(1).div_ceil(2),
            height.max(1).div_ceil(2),
        );
        let pong = Target::new(
            device,
            "bloom pong",
            width.max(1).div_ceil(2),
            height.max(1).div_ceil(2),
        );
        let bind = |source: &wgpu::TextureView, bloom: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post-process stage"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(bloom),
                    },
                ],
            })
        };
        let extract = bind(&scene.view, &scene.view);
        let horizontal = bind(&ping.view, &ping.view);
        let vertical = bind(&pong.view, &pong.view);
        let composite = bind(&scene.view, &ping.view);
        Targets {
            scene,
            _ping: ping,
            _pong: pong,
            extract,
            horizontal,
            vertical,
            composite,
        }
    }
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if size != (self.width, self.height) {
            self.targets = Self::targets(
                device,
                &self.layout,
                &self.sampler,
                &self.uniform,
                size.0,
                size.1,
            );
            (self.width, self.height) = size;
        }
    }
    pub fn scene_view(&self) -> &wgpu::TextureView {
        &self.targets.scene.view
    }

    /// Encode optional bloom and the final color transform. The output must match the
    /// format supplied to `new`; draw the ungraded UI after this call.
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        settings: Settings,
    ) {
        self.encode_to_viewport(
            queue,
            encoder,
            output,
            settings,
            (self.width, self.height),
            ViewportScale::Fill,
        );
    }

    /// Present the HDR scene at the output size without distorting its aspect ratio.
    /// UI can be drawn separately at the full output viewport size afterward.
    pub fn encode_to_viewport(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        settings: Settings,
        output_size: (u32, u32),
        mode: ViewportScale,
    ) {
        let mut packed = settings.uniform();
        let uv = viewport_uv_scale((self.width, self.height), output_size, mode);
        packed[13] = uv[0];
        packed[14] = uv[1];
        let offset = viewport_uv_offset((self.width, self.height), output_size, mode);
        packed[20] = offset[0];
        packed[21] = offset[1];
        packed[22] = (mode == ViewportScale::PixelPerfect) as u8 as f32;
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&packed));
        fn draw(
            encoder: &mut wgpu::CommandEncoder,
            label: &str,
            output: &wgpu::TextureView,
            pipeline: &wgpu::RenderPipeline,
            bind: &wgpu::BindGroup,
        ) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
        if settings.enabled && packed[8] > 0.0 {
            draw(
                encoder,
                "bloom extract",
                &self.targets._ping.view,
                &self.extract,
                &self.targets.extract,
            );
            draw(
                encoder,
                "bloom horizontal",
                &self.targets._pong.view,
                &self.horizontal,
                &self.targets.horizontal,
            );
            draw(
                encoder,
                "bloom vertical",
                &self.targets._ping.view,
                &self.vertical,
                &self.targets.vertical,
            );
        }
        draw(
            encoder,
            "image correction",
            output,
            &self.composite,
            &self.targets.composite,
        );
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::{viewport_uv_scale, ViewportScale};
    #[test]
    fn integer_scaling_handles_odd_bars_and_oversized_sources() {
        use super::viewport_uv_offset;
        let mode = ViewportScale::PixelPerfect;
        assert_eq!(viewport_uv_scale((2, 2), (5, 5), mode), [1.25, 1.25]);
        assert_eq!(viewport_uv_offset((2, 2), (5, 5), mode), [0.125, 0.125]);
        assert_eq!(viewport_uv_scale((8, 6), (5, 5), mode), [0.625, 5.0 / 6.0]);
        assert_eq!(
            viewport_uv_offset((8, 6), (5, 5), mode),
            [0.5 / 8.0, 0.5 / 6.0]
        );
    }
    #[test]
    fn fill_and_fit_preserve_aspect_for_wide_and_tall_outputs() {
        for (source, output) in [
            ((1920, 1080), (1200, 900)),
            ((1024, 768), (1920, 1080)),
            ((800, 800), (800, 800)),
        ] {
            let expected =
                (output.0 as f32 / output.1 as f32) / (source.0 as f32 / source.1 as f32);
            for mode in [ViewportScale::Fill, ViewportScale::Fit] {
                let [x, y] = viewport_uv_scale(source, output, mode);
                assert!((x / y - expected).abs() < 1e-6);
                match mode {
                    ViewportScale::Fill => assert!(x <= 1.0 && y <= 1.0 && (x == 1.0 || y == 1.0)),
                    ViewportScale::Fit => assert!(x >= 1.0 && y >= 1.0 && (x == 1.0 || y == 1.0)),
                    ViewportScale::PixelPerfect => unreachable!(),
                }
            }
        }
    }
}
