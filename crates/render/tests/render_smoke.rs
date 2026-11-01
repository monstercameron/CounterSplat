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
    stream.init_pool(&pool, &queue);
    stream.select([0.0, 0.0, 3.0], [0.0, 0.0, -1.0], None, 0.0);
    let requests = stream.requests();
    assert_eq!(requests.len(), 1);
    stream.insert(requests[0].0, bundle);
    stream.apply(&pool, &queue);
    assert_eq!(stream.stats().resident_splats, 1);
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

#[test]
fn target_cutouts_clear_opacity_for_the_direct_draw_path() {
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
        "fade readback",
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
    encoder.copy_buffer_to_buffer(&fade, 0, &readback, 0, 4);
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = slice.get_mapped_range().unwrap();
    assert_eq!(
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        0.0
    );
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
