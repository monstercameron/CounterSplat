//! Exercise the actual HDR bloom and correction pipelines on a GPU.
use countersplat_render::postprocess::{PostProcess, Settings, ViewportScale, SCENE_FORMAT};

fn render_to_viewport(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    post: &PostProcess,
    settings: Settings,
    width: u32,
    height: u32,
    mode: Option<ViewportScale>,
) -> Vec<u8> {
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("post test output"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: countersplat_render::COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("synthetic HDR source"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
  if abs(p.x - 32.0) < 2.0 && abs(p.y - 32.0) < 2.0 { return vec4f(4.0, 3.0, 2.0, 1.0); }
  if p.y < 20.0 { return vec4f(0.30, 0.10, 0.05, 1.0); }
  return vec4f(0.08, 0.10, 0.20, 1.0);
}
"#
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("synthetic HDR"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let row = (width * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("post readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("synthetic HDR source"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: post.scene_view(),
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
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    if let Some(mode) = mode {
        post.encode_to_viewport(
            queue,
            &mut encoder,
            &output.create_view(&Default::default()),
            settings,
            (width, height),
            mode,
        );
    } else {
        post.encode(
            queue,
            &mut encoder,
            &output.create_view(&Default::default()),
            settings,
        );
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap();
    let mut image = Vec::new();
    for y in 0..height as usize {
        image.extend_from_slice(&data[y * row as usize..y * row as usize + width as usize * 4]);
    }
    image
}
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    post: &PostProcess,
    settings: Settings,
    width: u32,
    height: u32,
) -> Vec<u8> {
    render_to_viewport(device, queue, post, settings, width, height, None)
}
fn pixel(image: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    image[(y * width + x) * 4..(y * width + x) * 4 + 4]
        .try_into()
        .unwrap()
}

#[test]
fn hdr_grade_bloom_bypass_and_resize_change_the_expected_pixels() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let bypass = render(&device, &queue, &post, Settings::default(), 64, 64);
    let upper = pixel(&bypass, 64, 5, 5);
    let lower = pixel(&bypass, 64, 5, 55);
    assert!(
        upper[0] > upper[2] && lower[2] > lower[0],
        "orientation or bypass changed: {upper:?} {lower:?}"
    );
    let mut grade = Settings {
        enabled: true,
        exposure_ev: 1.0,
        ..Default::default()
    };
    let exposed = render(&device, &queue, &post, grade, 64, 64);
    assert!(
        pixel(&exposed, 64, 5, 5)[0] > upper[0] + 20,
        "exposure did not brighten pixels"
    );
    grade.exposure_ev = 0.0;
    grade.saturation = 0.0;
    let monochrome = render(&device, &queue, &post, grade, 64, 64);
    let gray = pixel(&monochrome, 64, 5, 5);
    assert!(
        gray[0].abs_diff(gray[1]) <= 1 && gray[1].abs_diff(gray[2]) <= 1,
        "saturation: {gray:?}"
    );
    grade.saturation = 1.0;
    grade.filmic = true;
    let filmic = render(&device, &queue, &post, grade, 64, 64);
    assert!(
        pixel(&filmic, 64, 32, 32)[0] < 254,
        "filmic curve should roll off HDR highlights"
    );
    grade.filmic = false;
    grade.bloom_intensity = 1.0;
    let blooming = render(&device, &queue, &post, grade, 64, 64);
    let base = pixel(&bypass, 64, 38, 32);
    let glow = pixel(&blooming, 64, 38, 32);
    assert!(
        glow[0] > base[0] + 2,
        "bloom should spread into neighboring pixels: {base:?} {glow:?}"
    );
    // Bloom threshold is evaluated after exposure, not against an unrelated raw image.
    grade.exposure_ev = 1.0;
    grade.bloom_threshold = 5.0;
    grade.bloom_intensity = 0.0;
    let bright_without_bloom = render(&device, &queue, &post, grade, 64, 64);
    grade.bloom_intensity = 1.0;
    let bright_with_bloom = render(&device, &queue, &post, grade, 64, 64);
    assert!(
        pixel(&bright_with_bloom, 64, 38, 32)[0] > pixel(&bright_without_bloom, 64, 38, 32)[0] + 2
    );
    grade.exposure_ev = 8.0;
    grade.filmic = true;
    let extreme = render(&device, &queue, &post, grade, 64, 64);
    assert!(
        pixel(&extreme, 64, 32, 32)[0] > 200,
        "extreme HDR exposure must stay finite and bright"
    );
    post.resize(&device, 32, 16);
    let resized = render(&device, &queue, &post, grade, 32, 16);
    assert_eq!(resized.len(), 32 * 16 * 4);
}

#[test]
fn sharpening_increases_edge_contrast_and_preserves_flat_regions_and_bypass() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let neutral = Settings {
        enabled: true,
        ..Default::default()
    };
    let unsharpened = render(&device, &queue, &post, neutral, 64, 64);
    let sharpened = render(
        &device,
        &queue,
        &post,
        Settings {
            sharpness: 1.0,
            ..neutral
        },
        64,
        64,
    );
    let top = pixel(&unsharpened, 64, 5, 19);
    let lower = pixel(&unsharpened, 64, 5, 20);
    let top_sharp = pixel(&sharpened, 64, 5, 19);
    let lower_sharp = pixel(&sharpened, 64, 5, 20);
    assert!(
        top_sharp[0] > top[0],
        "bright edge should get sharper: {top:?} {top_sharp:?}"
    );
    assert!(
        lower_sharp[0] < lower[0],
        "dark edge should get sharper: {lower:?} {lower_sharp:?}"
    );
    assert_eq!(
        pixel(&unsharpened, 64, 5, 5),
        pixel(&sharpened, 64, 5, 5),
        "flat regions stay unchanged"
    );
    let bypass = render(&device, &queue, &post, Settings::default(), 64, 64);
    let disabled_sharp = render(
        &device,
        &queue,
        &post,
        Settings {
            sharpness: 2.0,
            ..Default::default()
        },
        64,
        64,
    );
    assert_eq!(
        bypass, disabled_sharp,
        "disabled correction must bypass sharpening"
    );
}

#[test]
fn presentation_fills_or_letterboxes_without_stretching_the_scene() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let fit = render_to_viewport(
        &device,
        &queue,
        &post,
        Settings::default(),
        128,
        64,
        Some(ViewportScale::Fit),
    );
    let fill = render_to_viewport(
        &device,
        &queue,
        &post,
        Settings::default(),
        128,
        64,
        Some(ViewportScale::Fill),
    );
    assert_eq!(pixel(&fit, 128, 2, 10)[0..3], [0, 0, 0]);
    assert_eq!(pixel(&fit, 128, 125, 10)[0..3], [0, 0, 0]);
    assert!(pixel(&fit, 128, 64, 10)[0] > 20);
    assert!(
        pixel(&fill, 128, 2, 10)[0] > 20,
        "fill should cover the viewport"
    );
    assert!(
        pixel(&fill, 128, 125, 55)[2] > 20,
        "fill should cover the opposite edge"
    );
}
