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
  if p.x < 5.0 && p.y < 16.0 { return vec4f(0.8, 0.8, 0.8, 1.0); }
  if abs(p.x - 32.0) < 2.0 && abs(p.y - 32.0) < 2.0 { return vec4f(4.0, 3.0, 2.0, 1.0); }
  if p.y < 20.0 && p.x > 40.0 { return vec4f(0.65, 0.65, 0.65, 1.0); }
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
        pixel(&unsharpened, 64, 10, 5),
        pixel(&sharpened, 64, 10, 5),
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

#[test]
fn pixel_perfect_replicates_texels_and_crops_oversized_scenes_without_filtering() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let grade = Settings {
        enabled: true,
        bloom_intensity: 0.2,
        grain: 0.04,
        vignette: 0.2,
        sharpness: 0.3,
        peripheral_blur: 0.6,
        tunnel_vision: 0.45,
        ..Default::default()
    };
    let image = render_to_viewport(
        &device,
        &queue,
        &post,
        grade,
        129,
        129,
        Some(ViewportScale::PixelPerfect),
    );
    for y in (0..128).step_by(2) {
        for x in (0..128).step_by(2) {
            let expected = pixel(&image, 129, x, y);
            assert_eq!(
                expected,
                pixel(&image, 129, x + 1, y),
                "horizontal texel at {x},{y}"
            );
            assert_eq!(
                expected,
                pixel(&image, 129, x, y + 1),
                "vertical texel at {x},{y}"
            );
            assert_eq!(
                expected,
                pixel(&image, 129, x + 1, y + 1),
                "texel block at {x},{y}"
            );
        }
    }
    assert_eq!(pixel(&image, 129, 128, 50)[0..3], [0, 0, 0]);
    let source = render(&device, &queue, &post, Settings::default(), 64, 64);
    let crop = render_to_viewport(
        &device,
        &queue,
        &post,
        Settings::default(),
        31,
        31,
        Some(ViewportScale::PixelPerfect),
    );
    for (x, y) in [(0, 0), (15, 15), (30, 30)] {
        assert_eq!(
            pixel(&crop, 31, x, y),
            pixel(&source, 64, x + 17, y + 17),
            "1x crop at {x},{y}"
        );
    }
}

#[test]
fn split_toning_cools_shadows_and_warms_highlights() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let neutral = Settings {
        enabled: true,
        ..Default::default()
    };
    let base = render(&device, &queue, &post, neutral, 64, 64);
    let toned = render(
        &device,
        &queue,
        &post,
        Settings {
            shadow_warmth: -1.0,
            highlight_warmth: 1.0,
            ..neutral
        },
        64,
        64,
    );
    let shadow = pixel(&base, 64, 5, 55);
    let cool = pixel(&toned, 64, 5, 55);
    let highlight = pixel(&base, 64, 50, 5);
    let warm = pixel(&toned, 64, 50, 5);
    assert!(
        cool[0] < shadow[0] && cool[2] > shadow[2],
        "shadow: {shadow:?} {cool:?}"
    );
    assert!(
        warm[0] > highlight[0] && warm[2] < highlight[2],
        "highlight: {highlight:?} {warm:?}"
    );
}

#[test]
fn sprint_peripheral_blur_softens_edge_detail_without_blurring_center() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let base = render(&device, &queue, &post, Settings::default(), 64, 64);
    let blurred = render(
        &device,
        &queue,
        &post,
        Settings {
            peripheral_blur: 1.0,
            ..Default::default()
        },
        64,
        64,
    );
    for (x, y) in [(31, 31), (32, 32), (30, 32), (33, 32)] {
        assert_eq!(
            pixel(&base, 64, x, y),
            pixel(&blurred, 64, x, y),
            "center must remain sharp"
        );
    }
    let changed = (1..15)
        .flat_map(|y| (2..8).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&base, 64, x, y) != pixel(&blurred, 64, x, y))
        .count();
    assert!(
        changed > 0,
        "blur should soften a high-contrast peripheral edge"
    );
}

#[test]
fn sustained_fire_vision_grows_at_the_edges_and_blends_with_sprint() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let post = PostProcess::new(&device, countersplat_render::COLOR_FORMAT, 64, 64);
    let base = render(&device, &queue, &post, Settings::default(), 64, 64);
    let light = render(
        &device,
        &queue,
        &post,
        Settings {
            tunnel_vision: 0.2,
            ..Default::default()
        },
        64,
        64,
    );
    let strong = render(
        &device,
        &queue,
        &post,
        Settings {
            tunnel_vision: 0.8,
            ..Default::default()
        },
        64,
        64,
    );
    let combined = render(
        &device,
        &queue,
        &post,
        Settings {
            tunnel_vision: 0.8,
            peripheral_blur: 0.5,
            ..Default::default()
        },
        64,
        64,
    );
    for (x, y) in [(30, 31), (31, 31), (32, 32), (33, 32)] {
        assert_eq!(
            pixel(&base, 64, x, y),
            pixel(&strong, 64, x, y),
            "tunnel vision must keep the aiming center clear"
        );
        assert_eq!(pixel(&base, 64, x, y), pixel(&combined, 64, x, y));
    }
    let difference = |image: &[u8]| -> u64 {
        (0..16)
            .flat_map(|y| (0..12).map(move |x| (x, y)))
            .map(|(x, y)| {
                let a = pixel(&base, 64, x, y);
                let b = pixel(image, 64, x, y);
                (0..3).map(|i| a[i].abs_diff(b[i]) as u64).sum::<u64>()
            })
            .sum()
    };
    let weak = difference(&light);
    let high = difference(&strong);
    let blended = difference(&combined);
    assert!(
        weak > 0 && high > weak && blended > high,
        "edge response: weak={weak} high={high} combined={blended}"
    );
}
