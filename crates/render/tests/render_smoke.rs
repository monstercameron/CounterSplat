//! End-to-end rendering check with a synthetic, brightly colored Gaussian.
use countersplat_assets::sog::{self, Bundle, Meta, Texture};
use countersplat_render::{
    camera,
    scene::{self, Pipelines, View},
};
use std::collections::BTreeMap;

fn single_splat() -> Bundle {
    let texture = |rgba: [u8; 4]| Texture {
        width: 1,
        height: 1,
        rgba: rgba.to_vec(),
    };
    let mut textures = BTreeMap::new();
    textures.insert("means_l", texture([0, 0, 0, 255]));
    textures.insert("means_u", texture([128, 128, 128, 255]));
    textures.insert("quats", texture([128, 128, 128, 252]));
    textures.insert("scales", texture([0, 1, 2, 255]));
    textures.insert("sh0", texture([0, 0, 0, 255]));
    let meta = Meta {
        count: 1,
        means_mins: [-1.0; 3],
        means_maxs: [1.0; 3],
        scales_codebook: (0..256).map(|i| -2.0 - i as f32 * 0.2).collect(),
        sh0_codebook: vec![1.0; 256],
        sh_n_codebook: vec![0.0; 256],
        sh_n_count: 0,
    };
    sog::complete(meta, textures)
}

#[test]
fn renders_visible_synthetic_splat() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter available; skipping render smoke test");
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let scene = scene::build(&device, &queue, &pipelines, &single_splat(), 0.5).unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 128,
        height: 128,
        near: 0.05,
        far: 100.0,
    };
    let image = scene.render_rgba(&device, &queue, &pipelines, &view, None);
    let lit_pixels = image
        .chunks_exact(4)
        .filter(|pixel| pixel[..3].iter().any(|&channel| channel > 5))
        .count();
    assert!(
        lit_pixels > 10,
        "expected a visible synthetic splat, got {lit_pixels} lit pixels"
    );
}

/// `n` small, varied splats spread over the cube [-1, 1]^3.
fn scattered_splats(n: u32) -> Bundle {
    let mut state = 7u32;
    let mut byte = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as u8
    };
    let stream = |f: &mut dyn FnMut(u32) -> [u8; 4]| Texture {
        width: n,
        height: 1,
        rgba: (0..n).flat_map(|i| f(i)).collect(),
    };
    let mut textures = BTreeMap::new();
    textures.insert("means_l", stream(&mut |_| [byte(), byte(), byte(), 255]));
    textures.insert("means_u", stream(&mut |_| [byte(), byte(), byte(), 255]));
    textures.insert("quats", stream(&mut |_| [byte(), byte(), byte(), 252]));
    textures.insert("scales", stream(&mut |_| [byte() / 4 + 150, byte() / 4 + 150, byte() / 4 + 150, 255]));
    textures.insert("sh0", stream(&mut |_| [byte(), byte(), byte(), byte() / 2 + 128]));
    let meta = Meta {
        count: n,
        means_mins: [-0.7; 3],
        means_maxs: [0.7; 3],
        scales_codebook: (0..256).map(|i| -6.0 + i as f32 * 0.02).collect(),
        sh0_codebook: (0..256).map(|i| -2.0 + i as f32 / 64.0).collect(),
        sh_n_codebook: vec![0.0; 256],
        sh_n_count: 0,
    };
    sog::complete(meta, textures)
}

/// The sort compacts the splats in view, so its order is defined only for that prefix; the draw must stop there even
/// inside the last, partly filled instance of 128. A previous view that showed more splats leaves stale entries past
/// the prefix, which would be drawn a second time. The compacting sort must give exactly the picture of the sort that
/// orders every slot.
#[test]
fn compacted_and_full_sorts_draw_the_same_picture() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let bundle = scattered_splats(5000);
    let compacting = scene::build(&device, &queue, &pipelines, &bundle, 0.5).unwrap();
    scene::set_indirect_draw(false);
    let full = scene::build(&device, &queue, &pipelines, &bundle, 0.5);
    scene::set_indirect_draw(true);
    let full = full.unwrap();
    let view = |eye: [f32; 3], target: [f32; 3]| View {
        model: camera::IDENTITY,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 96,
        height: 96,
        near: 0.05,
        far: 100.0,
    };
    // Everything in view, then from inside the cloud looking sideways: far fewer splats in view.
    for v in [view([0.0, 0.0, 3.0], [0.0; 3]), view([0.0, 0.0, 0.0], [1.0, 0.2, 0.0])] {
        let a = compacting.render_rgba(&device, &queue, &pipelines, &v, None);
        let b = full.render_rgba(&device, &queue, &pipelines, &v, None);
        assert!(a.chunks_exact(4).any(|p| p[..3].iter().any(|&c| c > 5)), "something is drawn");
        let differing = a.iter().zip(&b).filter(|(x, y)| x != y).count();
        assert_eq!(differing, 0, "compacted and full sorts differ in {differing} bytes");
    }
}

/// The sort leaves out splats the vertex stage would discard for their footprint (under `minPixelSize`, or off screen
/// at their full extent). That must not change a single pixel, with or without depth of field and motion blur, from
/// outside the cloud, inside it and close to splats, on a non-square viewport, and must actually leave splats out.
#[test]
fn footprint_cull_leaves_out_splats_without_changing_the_picture() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let n = 20_000u32;
    let mut state = 11u32;
    let mut byte = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as u8
    };
    let stream = |f: &mut dyn FnMut() -> [u8; 4]| Texture {
        width: 2048,
        height: n.div_ceil(2048),
        rgba: (0..n.div_ceil(2048) * 2048).flat_map(|_| f()).collect(),
    };
    let mut textures = BTreeMap::new();
    textures.insert("means_l", stream(&mut || [byte(), byte(), byte(), 255]));
    textures.insert("means_u", stream(&mut || [byte(), byte(), byte(), 255]));
    textures.insert("quats", stream(&mut || [byte(), byte(), byte(), 252 + byte() % 4]));
    // Scales from a tenth of a millimetre to about a metre, independently per axis (needles and discs as well).
    textures.insert("scales", stream(&mut || [byte(), byte(), byte(), 255]));
    textures.insert("sh0", stream(&mut || [byte(), byte(), byte(), byte() / 2 + 100]));
    let meta = Meta {
        count: n,
        means_mins: [-2.0; 3],
        means_maxs: [2.0; 3],
        scales_codebook: (0..256).map(|i| -9.0 + i as f32 * 0.035).collect(),
        sh0_codebook: (0..256).map(|i| -2.0 + i as f32 / 64.0).collect(),
        sh_n_codebook: vec![0.0; 256],
        sh_n_count: 0,
    };
    let scene = scene::build(&device, &queue, &pipelines, &sog::complete(meta, textures), 0.5).unwrap();
    let view = |eye: [f32; 3], target: [f32; 3], width: u32, height: u32| View {
        model: camera::IDENTITY,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(70f32.to_radians(), width as f32 / height as f32, 0.05, 100.0),
        width,
        height,
        near: 0.05,
        far: 100.0,
    };
    let views = [
        view([0.0, 0.0, 14.0], [0.0; 3], 160, 90),
        view([0.0, 0.0, 0.0], [1.0, 0.2, 0.0], 160, 90),
        view([0.3, -0.2, 0.5], [2.0, -0.5, -1.0], 96, 128),
    ];
    let mut dropped = 0;
    // Plain, then depth of field focused at 3 units with motion blur on.
    for (effects, previous) in [
        ([0.0, 7.5, 0.0, 0.0], camera::IDENTITY),
        ([0.0, 3.0, 6.0, 0.6], camera::mul(&views[1].projection, &views[1].view)),
    ] {
        scene.set_camera_effects(&queue, effects, &previous);
        for v in &views {
            countersplat_render::sort::set_footprint_cull(false);
            let full = scene.render_rgba(&device, &queue, &pipelines, v, None);
            let all = scene.drawn_splats(&device, &queue);
            countersplat_render::sort::set_footprint_cull(true);
            let culled = scene.render_rgba(&device, &queue, &pipelines, v, None);
            let kept = scene.drawn_splats(&device, &queue);
            assert!(full.chunks_exact(4).any(|p| p[..3].iter().any(|&c| c > 5)), "something is drawn");
            let differing = full.iter().zip(&culled).filter(|(x, y)| x != y).count();
            assert_eq!(differing, 0, "the footprint cull changed {differing} bytes (effects {effects:?})");
            assert!(kept <= all);
            dropped += all - kept;
        }
    }
    // Counts are whole draw instances of 128, so this is a floor on the splats left out over the six renders.
    assert!(dropped > 3_000, "the cull left out only {dropped} splats");
}

/// The impact depth prepass may leave out every splat farther than the farthest impact (`prepare_depth_range`): the
/// depth buffer must stay bit-identical wherever the full prepass is nearer than that limit, and may only be farther
/// elsewhere. Checked from outside the cloud and from inside it, with the limit cutting through the splats.
#[test]
fn depth_prepass_range_keeps_every_depth_nearer_than_the_limit() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let scene = scene::build(&device, &queue, &pipelines, &scattered_splats(5000), 0.5).unwrap();
    let (width, height) = (96u32, 96u32);
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("range test depth"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: countersplat_render::DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&Default::default());
    let row = (width * 4).div_ceil(256) * 256;
    let read_depth = |view: &View, far: Option<f32>| -> Vec<f32> {
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("range test readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        scene.prepare(&queue, &mut encoder, view, &view.model);
        scene.prepare_depth_range(&queue, &mut encoder, view, &view.model, far);
        {
            let mut pass = scene::depth_pass(&mut encoder, &depth_view, "range test prepass");
            scene.draw_depth(&mut pass, &pipelines);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        queue.submit(Some(encoder.finish()));
        readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let data = readback.slice(..).get_mapped_range().expect("mapped depth");
        (0..height)
            .flat_map(|y| {
                let start = (y * row) as usize;
                data[start..start + (width * 4) as usize]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let view = |eye: [f32; 3], target: [f32; 3]| View {
        model: camera::IDENTITY,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width,
        height,
        near: 0.05,
        far: 100.0,
    };
    // The cloud spans depths 2 to 4 from the first eye; its front half alone already covers every pixel the back half
    // does, so the first limit cuts through the front.
    for (v, far) in [(view([0.0, 0.0, 3.0], [0.0; 3]), 2.4f32), (view([0.0, 0.0, 0.0], [1.0, 0.2, 0.0]), 0.2)] {
        // The hardware depth of a point `far` in front of the camera.
        let p = &v.projection;
        let limit = (p[10] * -far + p[14]) / (p[11] * -far + p[15]);
        let full = read_depth(&v, None);
        let limited = read_depth(&v, Some(far));
        let (mut kept, mut dropped) = (0, 0);
        for (i, (&a, &b)) in full.iter().zip(&limited).enumerate() {
            if a < limit {
                assert_eq!(a.to_bits(), b.to_bits(), "pixel {i}: depth {a} nearer than the limit changed to {b}");
                kept += 1;
            } else {
                assert!(b >= a, "pixel {i}: depth {a} became nearer ({b})");
                dropped += usize::from(b > a);
            }
        }
        assert!(kept > 50, "the limit should keep near splats ({kept} pixels)");
        assert!(dropped > 50, "the limit should leave out far splats ({dropped} pixels)");
        // Without a limit the prepass is the full one again.
        let again = read_depth(&v, None);
        assert!(full.iter().zip(&again).all(|(a, b)| a.to_bits() == b.to_bits()), "an unlimited prepass is unchanged");
    }
}

#[test]
fn visible_splat_produces_a_draw_instance() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let buffer = |label: &str, size: u64, usage| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let centers = buffer(
        "centers",
        12,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    queue.write_buffer(&centers, 0, &[0; 12]);
    let fade = buffer("fade", 4, wgpu::BufferUsages::STORAGE);
    let sorter = countersplat_render::sort::DepthSorter::new(&device, &queue, 1, &centers, &fade);
    let order = buffer("order", 4, wgpu::BufferUsages::COPY_DST);
    let args = buffer(
        "args",
        20,
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let view = camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]);
    let projection = camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0);
    let clip = camera::mul(&projection, &view);
    let mut encoder = device.create_command_encoder(&Default::default());
    sorter.encode(&queue, &mut encoder, &view, Some(&clip), None, &order);
    encoder.copy_buffer_to_buffer(sorter.draw_args(), 0, &args, 0, 20);
    queue.submit(Some(encoder.finish()));
    let slice = args.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let values: Vec<_> = mapped
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    assert_eq!(
        values[1], 1,
        "visible geometry should create one draw instance: {values:?}"
    );
}

#[test]
fn streamed_splat_renders_and_respects_the_active_prefix() {
    use countersplat_assets::lod::{Leaf, LodMeta, Range};
    use countersplat_render::stream::{LevelStream, StreamSettings};
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter available; skipping streaming render test");
        return;
    };
    let bundle = single_splat();
    let chunk_meta = serde_json::to_vec(&serde_json::json!({
        "version": 2, "count": 1,
        "means": {"mins": bundle.meta.means_mins, "maxs": bundle.meta.means_maxs},
        "scales": {"codebook": bundle.meta.scales_codebook},
        "sh0": {"codebook": bundle.meta.sh0_codebook}
    }))
    .unwrap();
    let meta = LodMeta {
        counts: vec![1],
        filenames: vec!["0_0/meta.json".into()],
        leaves: vec![Leaf {
            min: [-1.0; 3],
            max: [1.0; 3],
            lods: vec![Some(Range {
                file: 0,
                offset: 0,
                count: 1,
            })],
        }],
    };
    let settings = StreamSettings {
        capacity: 128,
        fade_s: 0.0,
        ..Default::default()
    };
    let mut stream = LevelStream::new(meta, &[Some(chunk_meta)], settings).unwrap();
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let pool = scene::pool_with_centroid_rows(
        &device,
        &queue,
        &pipelines,
        stream.shared_meta(),
        128,
        0.5,
        1,
    )
    .unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 128,
        height: 128,
        near: 0.05,
        far: 100.0,
    };
    let lit = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[..3].iter().any(|&channel| channel > 5))
            .count()
    };
    stream.init_pool(&pool, &queue);
    // Sort the empty pool first: later sorts skip the fade writes, and the splat uploaded into an empty slot without a
    // crossfade (fade_s 0) must still find its share in place.
    assert_eq!(lit(&pool.render_rgba(&device, &queue, &pipelines, &view, None)), 0);
    assert_eq!(lit(&pool.render_rgba(&device, &queue, &pipelines, &view, None)), 0);
    stream.select([0.0, 0.0, 3.0], [0.0, 0.0, -1.0], None, 0.0);
    let requests = stream.requests();
    assert_eq!(requests.len(), 1);
    stream.insert(requests[0].0, bundle);
    stream.apply(&pool, &queue);
    assert_eq!(stream.stats().resident_splats, 1);
    assert!(lit(&pool.render_rgba(&device, &queue, &pipelines, &view, None)) > 10);
    pool.set_active(&queue, 0);
    assert_eq!(
        lit(&pool.render_rgba(&device, &queue, &pipelines, &view, None)),
        0
    );
}

#[test]
fn renders_spherical_splat_without_degenerate_eigenvector() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let mut bundle = single_splat();
    bundle.meta.scales_codebook = vec![-2.0; 256];
    let scene = scene::build(&device, &queue, &pipelines, &bundle, 0.5).unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 128,
        height: 128,
        near: 0.05,
        far: 100.0,
    };
    let pixels = scene.render_rgba(&device, &queue, &pipelines, &view, None);
    let lit = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3].iter().any(|&channel| channel > 5))
        .count();
    assert!(
        lit > 10,
        "a spherical Gaussian should render, got {lit} lit pixels"
    );
}

/// A splat between the camera and a cutout's target is left out of the draw: the compacting sort does not count it
/// in view. (A direct-draw sorter, which draws every slot, zeroes its fade instead; the sorter's unit tests check
/// both agree on which splats are shown.)
#[test]
fn target_cutouts_leave_the_cut_splat_out_of_the_draw() {
    use countersplat_render::sort::{Cutout, DepthSorter};
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let buffer = |label: &str, size: u64, usage| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let centers = buffer(
        "centers",
        12,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    queue.write_buffer(&centers, 0, &[0; 12]);
    let fade = buffer(
        "fade",
        4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let sorter = DepthSorter::new(&device, &queue, 1, &centers, &fade);
    let order = buffer("order", 4, wgpu::BufferUsages::COPY_DST);
    let readback = buffer(
        "visible readback",
        4,
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let view = camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]);
    let projection = camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0);
    let clip = camera::mul(&projection, &view);
    let cutout = Cutout {
        target: [0.0, 0.0, -1.0],
        radius: 1.0,
        floor_y: -1.0,
        depth_margin: 0.1,
        soft: 1.2,
        splat_reach: 0.0,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    sorter.encode(
        &queue,
        &mut encoder,
        &view,
        Some(&clip),
        Some((&cutout, &view, &camera::IDENTITY)),
        &order,
    );
    encoder.copy_buffer_to_buffer(sorter.visible_buffer(), 0, &readback, 0, 4);
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = slice.get_mapped_range().unwrap();
    assert_eq!(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), 0, "the cut splat is not in view");
}

#[test]
fn hdr_sky_composites_behind_the_splats() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let scene = scene::build(&device, &queue, &pipelines, &single_splat(), 0.5).unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 128,
        height: 128,
        near: 0.05,
        far: 100.0,
    };
    let cube = countersplat_render::env::Cubemap {
        size: 4,
        faces: std::array::from_fn(|_| vec![[0.0, 2.0, 0.0]; 16]),
    };
    let sky = countersplat_render::env::Sky::try_new(
        &device,
        &queue,
        &cube,
        countersplat_render::COLOR_FORMAT,
    )
    .unwrap();
    let with_sky = scene.render_rgba(&device, &queue, &pipelines, &view, Some((&sky, 1.0)));
    let without_sky = scene.render_rgba(&device, &queue, &pipelines, &view, None);
    assert!(
        with_sky[1] > 100 && without_sky[1] < 5,
        "sky should fill the background"
    );
    let splat_pixel = without_sky
        .chunks_exact(4)
        .enumerate()
        .max_by_key(|(_, p)| p[0])
        .unwrap()
        .0;
    assert!(
        without_sky[splat_pixel * 4] > 10 && with_sky[splat_pixel * 4] > 10,
        "splat should remain visible over the sky"
    );
}

#[test]
fn smooth_stream_keeps_visible_detail_when_the_pool_cannot_fit_both_lods() {
    use countersplat_assets::lod::{Leaf, LodMeta, Range};
    use countersplat_render::stream::{LevelStream, StreamSettings};

    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter available; skipping streaming transition test");
        return;
    };
    let make_bundle = |count: u32| {
        let mut bundle = single_splat();
        bundle.meta.count = count;
        for name in ["means_l", "means_u", "quats", "scales", "sh0"] {
            let texture = bundle.textures.get_mut(name).unwrap();
            texture.width = count;
            texture.rgba = texture.rgba.repeat(count as usize);
        }
        bundle
    };
    let fine = make_bundle(3);
    let coarse = make_bundle(2);
    let meta_json = |bundle: &Bundle| {
        serde_json::to_vec(&serde_json::json!({
            "version": 2, "count": bundle.meta.count,
            "means": {"mins": bundle.meta.means_mins, "maxs": bundle.meta.means_maxs},
            "scales": {"codebook": bundle.meta.scales_codebook},
            "sh0": {"codebook": bundle.meta.sh0_codebook}
        }))
        .unwrap()
    };
    let meta = LodMeta {
        counts: vec![3, 2],
        filenames: vec!["0/meta.json".into(), "1/meta.json".into()],
        leaves: vec![Leaf {
            min: [-1.0; 3],
            max: [1.0; 3],
            lods: vec![
                Some(Range {
                    file: 0,
                    offset: 0,
                    count: 3,
                }),
                Some(Range {
                    file: 1,
                    offset: 0,
                    count: 2,
                }),
            ],
        }],
    };
    let settings = StreamSettings {
        capacity: 4,
        fade_s: 0.25,
        allow_fade_cuts: false,
        ..Default::default()
    };
    let mut stream = LevelStream::new(
        meta,
        &[Some(meta_json(&fine)), Some(meta_json(&coarse))],
        settings,
    )
    .unwrap();
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let pool = scene::pool_with_centroid_rows(
        &device,
        &queue,
        &pipelines,
        stream.shared_meta(),
        4,
        0.5,
        1,
    )
    .unwrap();
    stream.init_pool(&pool, &queue);

    stream.set_fixed_lod(Some(1)).unwrap();
    stream.select([0.0, 0.0, 3.0], [0.0, 0.0, -1.0], None, 0.0);
    stream.insert(1, coarse);
    stream.apply(&pool, &queue);
    assert_eq!(stream.stats().resident_splats, 2);

    stream.insert(0, fine);
    stream.set_fixed_lod(Some(0)).unwrap();
    stream.select([0.0, 0.0, 3.0], [0.0, 0.0, -1.0], None, 1.0);
    stream.apply(&pool, &queue);
    let stats = stream.stats();
    assert_eq!(stats.resident_splats, 2, "keep the existing visible chunk");
    assert_eq!(
        stats.leaves_pending, 1,
        "request remains pending until room exists"
    );
    assert_eq!(
        stats.hard_swaps, 0,
        "smooth mode must not abruptly replace it"
    );

    // With fades explicitly disabled, a full pool can swap in one frame.
    stream.settings.fade_s = 0.0;
    stream.select([0.0, 0.0, 3.0], [0.0, 0.0, -1.0], None, 2.0);
    stream.apply(&pool, &queue);
    assert_eq!(stream.stats().resident_splats, 3);
    assert_eq!(stream.stats().hard_swaps, 1);
}

#[test]
fn rapid_lod_changes_finish_the_active_crossfade_before_replacing_it() {
    use countersplat_assets::lod::{Leaf, LodMeta, Range};
    use countersplat_render::stream::{LevelStream, StreamSettings};

    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter available; skipping streaming transition test");
        return;
    };
    let sample = single_splat();
    let metadata = serde_json::to_vec(&serde_json::json!({
        "version": 2, "count": 1,
        "means": {"mins": sample.meta.means_mins, "maxs": sample.meta.means_maxs},
        "scales": {"codebook": sample.meta.scales_codebook},
        "sh0": {"codebook": sample.meta.sh0_codebook}
    }))
    .unwrap();
    let meta = LodMeta {
        counts: vec![1, 1, 1],
        filenames: vec![
            "0/meta.json".into(),
            "1/meta.json".into(),
            "2/meta.json".into(),
        ],
        leaves: vec![Leaf {
            min: [-1.0; 3],
            max: [1.0; 3],
            lods: (0..3)
                .map(|file| {
                    Some(Range {
                        file,
                        offset: 0,
                        count: 1,
                    })
                })
                .collect(),
        }],
    };
    let settings = StreamSettings {
        capacity: 8,
        fade_s: 0.4,
        allow_fade_cuts: false,
        ..Default::default()
    };
    let mut stream = LevelStream::new(
        meta,
        &[
            Some(metadata.clone()),
            Some(metadata.clone()),
            Some(metadata),
        ],
        settings,
    )
    .unwrap();
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let pool = scene::pool_with_centroid_rows(
        &device,
        &queue,
        &pipelines,
        stream.shared_meta(),
        8,
        0.5,
        1,
    )
    .unwrap();
    stream.init_pool(&pool, &queue);
    for file in 0..3 {
        stream.insert(file, single_splat());
    }
    let eye = [0.0, 0.0, 3.0];
    let forward = [0.0, 0.0, -1.0];
    stream.set_fixed_lod(Some(2)).unwrap();
    stream.select(eye, forward, None, 0.0);
    stream.apply(&pool, &queue);

    stream.set_fixed_lod(Some(1)).unwrap();
    stream.select(eye, forward, None, 1.0);
    stream.apply(&pool, &queue);
    assert_eq!(stream.stats().leaves_per_lod[1], 1);

    stream.set_fixed_lod(Some(0)).unwrap();
    stream.select(eye, forward, None, 1.1);
    stream.apply(&pool, &queue);
    let held = stream.stats();
    assert_eq!(
        held.leaves_per_lod[1], 1,
        "the in-progress fade keeps its current LOD"
    );
    assert_eq!(held.leaves_pending, 1);
    let weights: Vec<f32> = stream.fades().iter().map(|fade| fade.2).collect();
    assert_eq!(weights.len(), 2);
    assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);

    stream.select(eye, forward, None, 1.5);
    stream.apply(&pool, &queue);
    assert_eq!(
        stream.stats().leaves_per_lod[0],
        1,
        "the new target follows after the fade"
    );
}

#[test]
fn lens_exposure_focus_and_shutter_change_gpu_output() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let scene = scene::build(&device, &queue, &pipelines, &single_splat(), 0.5).unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 128,
        height: 128,
        near: 0.05,
        far: 100.0,
    };
    let metrics = |pixels: &[u8]| {
        let mut sum = 0.0f64;
        let mut sx = 0.0f64;
        let mut sy = 0.0f64;
        let mut x2 = 0.0f64;
        let mut y2 = 0.0f64;
        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
            let weight = f64::from(pixel[0]) + f64::from(pixel[1]) + f64::from(pixel[2]);
            let x = (index % 128) as f64;
            let y = (index / 128) as f64;
            sum += weight;
            sx += weight * x;
            sy += weight * y;
            x2 += weight * x * x;
            y2 += weight * y * y;
        }
        (
            sum,
            x2 / sum - (sx / sum).powi(2),
            y2 / sum - (sy / sum).powi(2),
        )
    };
    let render = || scene.render_rgba(&device, &queue, &pipelines, &view, None);
    let baseline = metrics(&render());
    scene.set_camera_effects(&queue, [1.0, 0.0, 0.0, 0.0], &camera::IDENTITY);
    let brighter = metrics(&render());
    assert!(
        brighter.0 > baseline.0 * 1.1,
        "exposure should brighten the rendered splat: {baseline:?} vs {brighter:?}"
    );
    scene.set_camera_effects(&queue, [0.0, 1.0, 20.0, 0.0], &camera::IDENTITY);
    let defocused = metrics(&render());
    assert!(
        defocused.1 > baseline.1 && defocused.2 > baseline.2,
        "out-of-focus splats should spread in both axes: {baseline:?} vs {defocused:?}"
    );
    let previous_view = camera::look_at([-1.0, 0.0, 3.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let previous_clip = camera::mul(&view.projection, &previous_view);
    scene.set_camera_effects(&queue, [0.0, 0.0, 0.0, 1.0], &previous_clip);
    let motion = metrics(&render());
    assert!(motion.1 > baseline.1 && motion.1 > motion.2,
        "camera translation should spread the splat in the motion direction: {baseline:?} vs {motion:?}");

    // An unchanged camera stays sharp even when its shutter spans multiple frames.
    let stationary_clip = camera::mul(&view.projection, &view.view);
    scene.set_camera_effects(&queue, [0.0, 0.0, 0.0, 2.5], &stationary_clip);
    let stationary = metrics(&render());
    assert!(
        (stationary.1 - baseline.1).abs() < 0.01 && (stationary.2 - baseline.2).abs() < 0.01,
        "stationary view should not smear: {baseline:?} vs {stationary:?}"
    );

    let mut cadence_widths = Vec::new();
    for fps in [60.0_f32, 120.0] {
        let delta = 1.0 / fps;
        let distance = 24.0 * delta;
        let old_view = camera::look_at(
            [-distance, 0.0, 3.0],
            [-distance, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        let old_clip = camera::mul(&view.projection, &old_view);
        let exposure_scale = camera::shutter_motion_scale(1.0 / 48.0, delta);
        scene.set_camera_effects(&queue, [0.0, 0.0, 0.0, exposure_scale], &old_clip);
        cadence_widths.push(metrics(&render()).1);
    }
    assert!((cadence_widths[0] - cadence_widths[1]).abs() < 0.1,
        "the same shutter and velocity should produce the same blur at either frame rate: {cadence_widths:?}");
}

#[test]
fn local_light_brightens_only_nearby_splats_and_preserves_camera_effects() {
    use countersplat_render::scene::LocalLight;
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT).unwrap();
    let scene = scene::build(&device, &queue, &pipelines, &single_splat(), 0.5).unwrap();
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0),
        width: 128,
        height: 128,
        near: 0.05,
        far: 100.0,
    };
    let render = || scene.render_rgba(&device, &queue, &pipelines, &view, None);
    let brightness = |pixels: &[u8]| -> u64 {
        pixels
            .chunks_exact(4)
            .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
            .sum()
    };
    let baseline = render();
    let light = LocalLight {
        position: [0.0, 0.0, 0.0],
        radius: 2.0,
        color: [1.0, 0.68, 0.35],
        intensity: 0.6,
    };
    scene.set_local_light(&queue, Some(light));
    let nearby = render();
    assert!(
        brightness(&nearby) > brightness(&baseline) * 105 / 100,
        "nearby splat should brighten"
    );
    scene.set_camera_effects(&queue, [0.0, 0.0, 0.0, 0.0], &camera::IDENTITY);
    assert_eq!(nearby, render(), "camera updates must preserve the light");
    scene.set_local_light(
        &queue,
        Some(LocalLight {
            position: [20.0, 0.0, 0.0],
            ..light
        }),
    );
    assert_eq!(
        baseline,
        render(),
        "a light outside its radius must not affect the splat"
    );
    scene.set_local_light(
        &queue,
        Some(LocalLight {
            radius: f32::NAN,
            ..light
        }),
    );
    assert_eq!(
        baseline,
        render(),
        "invalid light data must disable the effect"
    );
    scene.set_local_light(&queue, None);
    assert_eq!(baseline, render());
}
