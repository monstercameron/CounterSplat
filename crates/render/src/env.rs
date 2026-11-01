//! Sky backgrounds: an equirectangular HDR (a Radiance `.hdr` file, or a procedural sky) converted
//! to a cubemap on the CPU, then drawn as the background behind the splats.
//!
//! Cubemap faces use the OpenGL/WebGPU layer order (+X, −X, +Y, −Y, +Z, −Z) with the texel grid starting at the
//! top-left, so `textureSample` on the GPU returns the environment in the direction it was sampled from.

use crate::camera::{self, Mat4};
pub use countersplat_assets::environment::Equirect;
use std::io::Read;
use std::path::Path;

/// Six square faces of linear RGB, `size × size` texels each, in layer order.
#[derive(Debug, Clone)]
pub struct Cubemap {
    pub size: usize,
    pub faces: [Vec<[f32; 3]>; 6],
}

impl Cubemap {
    /// Validate all six faces before converting or uploading them.
    pub fn validate(&self) -> Result<(), &'static str> {
        let Some(count) = self.size.checked_mul(self.size) else {
            return Err("cubemap size overflows");
        };
        if !(1..=2048).contains(&self.size) || self.faces.iter().any(|face| face.len() != count) {
            return Err("cubemap must have six complete square faces");
        }
        if !self
            .faces
            .iter()
            .flatten()
            .flatten()
            .all(|value| value.is_finite() && *value >= 0.0)
        {
            return Err("cubemap radiance must be finite and nonnegative");
        }
        Ok(())
    }
}

/// The procedural sky: a blue gradient over a dark ground, with a sun disc and glow. Linear HDR values.
pub fn procedural(width: usize, height: usize, sun: [f32; 3]) -> Equirect {
    let len = (sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2])
        .sqrt()
        .max(1e-6);
    let s = [sun[0] / len, sun[1] / len, sun[2] / len];
    let mut rgb = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let d = direction_of_equirect(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            );
            let up = d[1];
            let sky = mix([0.55, 0.68, 0.9], [0.18, 0.36, 0.78], up.max(0.0).sqrt());
            let ground = [0.07, 0.065, 0.06];
            let mut color = if up >= 0.0 { sky } else { ground };
            let cos_sun = (d[0] * s[0] + d[1] * s[1] + d[2] * s[2]).max(0.0);
            let disc = if cos_sun > 0.9995 { 60.0 } else { 0.0 };
            let glow = cos_sun.powf(64.0) * 2.5;
            for c in 0..3 {
                color[c] += disc * [1.0, 0.96, 0.88][c] + glow * [1.0, 0.9, 0.75][c];
            }
            rgb.push(color);
        }
    }
    Equirect { width, height, rgb }
}

/// Loads a Radiance `.hdr` panorama. The image must be 2:1 (width is twice the height).
pub fn load_hdr(path: &Path) -> Result<Equirect, Box<dyn std::error::Error>> {
    // Bound the read itself, rather than allocating an arbitrarily large file before validation.
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take((countersplat_assets::environment::MAX_ENCODED_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    countersplat_assets::environment::decode_hdr(&bytes).map_err(Into::into)
}

/// Validate an untrusted panorama and bounded face size before cubemap conversion.
pub fn try_to_cubemap(
    panorama: &Equirect,
    face_size: usize,
    rotation_degrees: f32,
) -> Result<Cubemap, String> {
    panorama.validate().map_err(str::to_owned)?;
    if !(1..=2048).contains(&face_size)
        || !rotation_degrees.is_finite()
        || rotation_degrees.abs() > 1_000_000.0
    {
        return Err("cubemap size or rotation is invalid".into());
    }
    Ok(to_cubemap(panorama, face_size, rotation_degrees))
}

/// Converts a panorama to a cubemap. `rotation_degrees` spins the environment around +Y.
pub fn to_cubemap(panorama: &Equirect, face_size: usize, rotation_degrees: f32) -> Cubemap {
    let spin = rotation_degrees.to_radians();
    let (sin, cos) = spin.sin_cos();
    let faces = std::array::from_fn(|face| {
        let mut texels = Vec::with_capacity(face_size * face_size);
        for y in 0..face_size {
            for x in 0..face_size {
                let u = 2.0 * (x as f32 + 0.5) / face_size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / face_size as f32 - 1.0;
                let d = face_direction(face, u, v);
                // Rotate the lookup direction about +Y: the environment turns the opposite way.
                let r = [cos * d[0] - sin * d[2], d[1], sin * d[0] + cos * d[2]];
                let (eu, ev) = equirect_of_direction(r);
                texels.push(sample_bilinear(panorama, eu, ev));
            }
        }
        texels
    });
    Cubemap {
        size: face_size,
        faces,
    }
}

/// The unit direction through the centre of a texel on a cube face, `(u, v)` in [-1, 1] with v pointing down.
pub fn face_direction(face: usize, u: f32, v: f32) -> [f32; 3] {
    let d = match face {
        0 => [1.0, -v, -u],
        1 => [-1.0, -v, u],
        2 => [u, 1.0, v],
        3 => [u, -1.0, -v],
        4 => [u, -v, 1.0],
        _ => [-u, -v, -1.0],
    };
    normalize(d)
}

/// Panorama coordinates `(u, v)` in [0, 1) for a unit direction. Forward (−Z) is the centre of the image.
fn equirect_of_direction(d: [f32; 3]) -> (f32, f32) {
    let u = 0.5 + d[0].atan2(-d[2]) / (2.0 * std::f32::consts::PI);
    let v = d[1].clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
    (u.rem_euclid(1.0), v)
}

/// The unit direction for a panorama coordinate: the inverse of [`equirect_of_direction`].
fn direction_of_equirect(u: f32, v: f32) -> [f32; 3] {
    let azimuth = (u - 0.5) * 2.0 * std::f32::consts::PI;
    let elevation = std::f32::consts::FRAC_PI_2 - v * std::f32::consts::PI;
    let (se, ce) = elevation.sin_cos();
    [ce * azimuth.sin(), se, -ce * azimuth.cos()]
}

fn sample_bilinear(panorama: &Equirect, u: f32, v: f32) -> [f32; 3] {
    let fx = u * panorama.width as f32 - 0.5;
    let fy = (v * panorama.height as f32 - 0.5).clamp(0.0, (panorama.height - 1) as f32);
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let w = panorama.width as i64;
    let h = panorama.height as i64;
    let texel = |x: i64, y: i64| panorama.rgb[(y.clamp(0, h - 1) * w + x.rem_euclid(w)) as usize];
    let (x0i, y0i) = (x0 as i64, y0 as i64);
    let top = mix(texel(x0i, y0i), texel(x0i + 1, y0i), tx);
    let bottom = mix(texel(x0i, y0i + 1), texel(x0i + 1, y0i + 1), tx);
    mix(top, bottom, ty)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

/// IEEE half-precision bits for `value`, truncated (not rounded) and clamped to the finite range.
pub fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    if value.is_nan() {
        return sign | 0x7e00;
    }
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x007f_ffff;
    if exponent >= 31 {
        return sign | 0x7bff; // clamp to the largest finite half rather than infinity
    }
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let shifted = (mantissa | 0x0080_0000) >> (14 - exponent);
        return sign | shifted as u16;
    }
    sign | ((exponent as u16) << 10) | (mantissa >> 13) as u16
}

/// Converts a cubemap to RGBA half-float texels, the layout the GPU texture uses. Alpha is 1.
pub fn cubemap_rgba16f(cubemap: &Cubemap) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(cubemap.size * cubemap.size * 6 * 8);
    for face in &cubemap.faces {
        for texel in face {
            for value in [texel[0], texel[1], texel[2], 1.0] {
                bytes.extend_from_slice(&f32_to_f16_bits(value).to_le_bytes());
            }
        }
    }
    bytes
}

/// The GPU sky: samples the cubemap along each pixel's view ray. HDR targets receive linear radiance; display targets use the legacy tone map.
pub struct Sky {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    pub size: usize,
}

/// Bytes of the sky uniform: two `mat4x4f` and the exposure, padded to the WGSL struct size.
const SKY_UNIFORM_SIZE: u64 = 144;

impl Sky {
    /// Uploads `cubemap` and builds the sky pipeline for a colour target of `color_format`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cubemap: &Cubemap,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        Self::try_new(device, queue, cubemap, color_format)
            .expect("invalid cubemap passed to Sky::new")
    }

    /// Validate and upload six linear-light cubemap faces.
    pub fn try_new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cubemap: &Cubemap,
        color_format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        cubemap.validate().map_err(str::to_owned)?;
        Self::from_rgba16f(
            device,
            queue,
            cubemap.size,
            &cubemap_rgba16f(cubemap),
            color_format,
        )
    }

    /// Upload already-converted RGBA16F cube faces. Useful for decoding on a separate worker.
    /// The input must contain six complete square faces in +X, -X, +Y, -Y, +Z, -Z order.
    pub fn from_rgba16f(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: usize,
        bytes: &[u8],
        color_format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let expected = size.checked_mul(size).and_then(|n| n.checked_mul(6 * 8));
        if size == 0
            || size > 2048
            || size > device.limits().max_texture_dimension_2d as usize
            || expected != Some(bytes.len())
        {
            return Err("invalid cubemap size or RGBA16F byte length".into());
        }
        if bytes.chunks_exact(2).any(|v| {
            let bits = u16::from_le_bytes([v[0], v[1]]);
            bits & 0x7c00 == 0x7c00 || bits & 0x8000 != 0
        }) {
            return Err("cubemap radiance must be finite and nonnegative".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment cubemap"),
            size: wgpu::Extent3d {
                width: size as u32,
                height: size as u32,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size as u32 * 8),
                rows_per_image: Some(size as u32),
            },
            wgpu::Extent3d {
                width: size as u32,
                height: size as u32,
                depth_or_array_layers: 6,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("environment sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky uniform"),
            size: SKY_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // HDR hosts retain radiance for post-processing; display hosts tone-map directly.
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sky.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if color_format == crate::postprocess::SCENE_FORMAT {
                    "fs_linear_hdr"
                } else {
                    "fs_main"
                }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                // The sky sits at the far plane and never writes depth, so the splats draw over it.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self {
            pipeline,
            bind_group,
            uniform,
            size,
        })
    }

    /// Records the sky into `pass`. `view` is the world-to-view matrix and `projection` the view-to-clip matrix.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: &Mat4,
        projection: &Mat4,
        exposure: f32,
    ) {
        self.draw_rotated(queue, pass, view, projection, exposure, 0.0);
    }

    /// Draw the sky with a runtime world-Y rotation, without rebuilding the cubemap.
    pub fn draw_rotated(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: &Mat4,
        projection: &Mat4,
        exposure: f32,
        rotation_degrees: f32,
    ) {
        let inverse_projection = camera::inverse(projection).unwrap_or(camera::IDENTITY);
        let mut bytes = Vec::with_capacity(SKY_UNIFORM_SIZE as usize);
        for value in inverse_projection.iter().chain(view.iter()) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let (sin, cos) = rotation_degrees.to_radians().sin_cos();
        for value in [exposure, sin, cos] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.resize(SKY_UNIFORM_SIZE as usize, 0);
        queue.write_buffer(&self.uniform, 0, &bytes);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_match_the_ieee_layout_for_common_values() {
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(0.5), 0x3800);
        assert_eq!(f32_to_f16_bits(-2.0), 0xc000);
        assert_eq!(f32_to_f16_bits(65504.0), 0x7bff);
        assert_eq!(f32_to_f16_bits(1.0e9), 0x7bff);
    }

    #[test]
    fn equirect_and_direction_are_inverses() {
        for (u, v) in [(0.5, 0.5), (0.25, 0.3), (0.8, 0.9), (0.05, 0.1)] {
            let d = direction_of_equirect(u, v);
            let (u2, v2) = equirect_of_direction(d);
            assert!(
                (u - u2).abs() < 1e-4 && (v - v2).abs() < 1e-4,
                "{u},{v} -> {u2},{v2}"
            );
        }
    }

    #[test]
    fn the_centre_of_each_cube_face_points_along_its_axis() {
        let expected = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        for (face, want) in expected.iter().enumerate() {
            let d = face_direction(face, 0.0, 0.0);
            for i in 0..3 {
                assert!((d[i] - want[i]).abs() < 1e-6, "face {face}: {d:?}");
            }
        }
    }

    #[test]
    fn a_uniform_panorama_becomes_a_uniform_cubemap() {
        let panorama = Equirect {
            width: 8,
            height: 4,
            rgb: vec![[0.25, 0.5, 2.0]; 32],
        };
        let cube = to_cubemap(&panorama, 4, 37.0);
        assert_eq!(cube.size, 4);
        for face in &cube.faces {
            for texel in face {
                assert!((texel[2] - 2.0).abs() < 1e-5 && (texel[0] - 0.25).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn procedural_sky_is_brighter_at_the_sun_than_the_ground() {
        let sky = procedural(64, 32, [0.0, 1.0, 0.0]);
        let zenith_row = &sky.rgb[..64];
        let nadir_row = &sky.rgb[31 * 64..];
        let max_top = zenith_row.iter().map(|c| c[0]).fold(0.0, f32::max);
        let max_bottom = nadir_row.iter().map(|c| c[0]).fold(0.0, f32::max);
        assert!(max_top > max_bottom, "sky {max_top} vs ground {max_bottom}");
    }
}
