//! Streaming benchmark: flies a camera through a streamed (multi-LOD) level and reports what the loader did.
//!
//! The level is driven the way the browser host drives it: the same `StreamSettings`, `select` /
//! `requests_up_to` / `insert` / `apply` calls every frame, at a fixed 60 Hz simulation step, with four decode
//! workers whose latency is each chunk's real decode time. Nothing depends on wall-clock frame pacing, so two runs
//! of the same build agree and two builds can be compared.
//!
//! ```text
//! cargo run --release -p countersplat-render --example stream_bench -- assets/levels/<id> [seconds] [capacity]
//! ```
//!
//! It reports pop-in proxies (LOD switches, swaps without a crossfade, how often a leaf in view had nothing to
//! draw), memory (resident splats, decoded chunks held, bytes uploaded) and per-frame stream cost.
use countersplat_assets::{lod::LodMeta, sog, viewer::CameraHint};
use countersplat_render::{
    camera,
    scene::{self, color_pass, Pipelines, View},
    stream::{DrawRegion, LevelStream, StreamSettings},
    COLOR_FORMAT, DEPTH_FORMAT,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

const DT: f32 = 1.0 / 60.0;
const WORKERS: usize = 4;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

/// The browser host's settings (`apps/web/src/lib.rs`).
fn browser_settings(capacity: u32, chunks: usize) -> StreamSettings {
    StreamSettings {
        capacity,
        max_distance_m: 150.0,
        draw_region: DrawRegion::ForwardEllipse {
            rear_fraction: 0.3,
            side_fraction: 0.55,
        },
        fade_s: 0.45,
        keep_s: 2.5,
        near_lod0_m: 12.0,
        center_lod0_m: 1000.0,
        offscreen_keep_m: 12.0,
        allow_fade_cuts: false,
        max_in_flight: 4,
        cache_chunks: chunks.min(16),
        ..Default::default()
    }
}

/// `set=name:value,...` on the command line: the `StreamSettings` fields worth tuning without a rebuild.
fn apply_overrides(settings: &mut StreamSettings, spec: &str) {
    for item in spec.split(',') {
        let (name, value) = item.split_once(':').expect("set=name:value");
        let number: f32 = value.parse().expect("numeric setting");
        match name {
            "lod_error_px" => settings.lod_error_px = number,
            "lod_distance_m" => settings.lod_distance_m = number,
            "center_lod0_m" => settings.center_lod0_m = number,
            "near_lod0_m" => settings.near_lod0_m = number,
            "offscreen_keep_m" => settings.offscreen_keep_m = number,
            "max_distance_m" => settings.max_distance_m = number,
            "behind_bias" => settings.behind_bias = number as usize,
            "uploads_per_frame" => settings.uploads_per_frame = number as u32,
            "cache_chunks" => settings.cache_chunks = number as usize,
            "fade_s" => settings.fade_s = number,
            "keep_s" => settings.keep_s = number,
            "any_view_m" => settings.any_view_m = number,
            "select_share_percent" => settings.select_share_percent = number as u32,
            other => panic!("unknown setting {other}"),
        }
    }
}

struct Pending {
    file: usize,
    arrives: u64,
    bundle: Option<sog::Bundle>,
}

/// A second stream over the same level that always shows every region at its finest LOD with room for all of it,
/// fully loaded: what the live frame would look like if loading were free. The live frame is scored against it.
struct Oracle {
    stream: LevelStream,
    pool: scene::GsplatScene,
    now: f32,
}

impl Oracle {
    fn new(
        dir: &Path,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &Pipelines,
        base: &StreamSettings,
        lod0_total: u64,
        focal_px: f32,
    ) -> Self {
        let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
        let metas: Vec<Option<Vec<u8>>> = meta
            .filenames
            .iter()
            .map(|name| std::fs::read(dir.join(name)).ok())
            .collect();
        let settings = StreamSettings {
            // The selection spends only 85% of the pool, and every leaf is charged its coarsest LOD first, so the
            // pool needs real headroom over the whole LOD 0 set for every leaf to reach it.
            // A pool's per-splat centre buffer is 12 bytes a splat and one storage binding is limited to 128 MiB on a
            // default device, so a pool cannot exceed about 11.1 million splats.
            capacity: (((lod0_total as f64 * 1.3) as u32).saturating_add(1_000_000))
                .min(11_000_000),
            cache_chunks: metas.len(),
            max_in_flight: metas.len(),
            // The oracle is not rate-limited: upload whatever each frame needs.
            uploads_per_frame: u32::MAX / 2,
            fade_s: 0.0,
            allow_fade_cuts: true,
            ..*base
        };
        let mut stream = LevelStream::new(meta, &metas, settings).expect("oracle stream");
        stream.set_focal_px(focal_px);
        stream.set_fixed_lod(Some(0)).expect("fixed lod 0");
        let rows = stream.palette_texture_rows(device.limits().max_texture_dimension_2d);
        let pool = scene::pool_for_level(
            device,
            queue,
            pipelines,
            stream.shared_meta(),
            settings.capacity,
            0.5,
            rows,
        )
        .expect("oracle pool");
        stream.init_pool(&pool, queue);
        Self {
            stream,
            pool,
            now: 0.0,
        }
    }

    /// Streams in everything the pose needs, instantly, until nothing is pending.
    fn show(
        &mut self,
        dir: &Path,
        queue: &wgpu::Queue,
        eye: [f32; 3],
        forward: [f32; 3],
        clip: &[f32; 16],
    ) {
        for step in 0..400 {
            self.now += DT;
            self.stream.select(eye, forward, Some(clip), self.now);
            for (file, folder) in self.stream.requests() {
                match sog::load_dir(&dir.join(&folder)) {
                    Ok(bundle) => self.stream.insert(file, bundle),
                    Err(_) => self.stream.failed(file),
                }
            }
            self.stream.apply(&self.pool, queue);
            let s = self.stream.stats();
            // Leaves waiting for their first data are counted apart from ones waiting for a different LOD.
            if step > 2
                && s.leaves_pending == 0
                && s.leaves_empty == 0
                && s.chunks_in_flight == 0
                && s.fading == 0
            {
                break;
            }
        }
    }
}

/// Mean squared error over the RGB channels of the pixels where either image has content (some channel above 10).
/// Both images are black over a scan's empty background, which would otherwise make every frame look near-perfect.
fn content_mse(a: &[u8], b: &[u8]) -> f64 {
    let mut sum = 0f64;
    let mut n = 0f64;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        if p[..3].iter().chain(&q[..3]).all(|&c| c <= 10) {
            continue;
        }
        for c in 0..3 {
            let d = f64::from(p[c]) - f64::from(q[c]);
            sum += d * d;
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

fn psnr_of(mse: f64) -> f64 {
    if mse < 1e-9 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

fn bundle_bytes(bundle: &sog::Bundle) -> usize {
    bundle.textures.values().map(|t| t.rgba.len()).sum()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("level directory (assets/levels/<id>)"));
    // Named options, `key=value`:
    //   seconds=90 capacity=3000000 speed=3        the run and the pool
    //   loop=x,z,radius                            walk a circle instead of straight ahead from the viewer camera
    //                                              (the dense part of a scan is often small and far from the start)
    //   quality=N                                  compare N frames against a full-detail oracle (PSNR)
    //   set=name:value,name:value                  override StreamSettings fields (see `apply_overrides`)
    let mut options: BTreeMap<String, String> = BTreeMap::new();
    for arg in args {
        let (key, value) = arg.split_once('=').expect("options are key=value");
        options.insert(key.to_owned(), value.to_owned());
    }
    let opt = |key: &str| options.get(key).map(String::as_str);
    let seconds: f32 = opt("seconds").map_or(60.0, |v| v.parse().expect("seconds"));
    let capacity: u32 = opt("capacity").map_or(3_000_000, |v| v.parse().expect("capacity"));
    let speed: f32 = opt("speed").map_or(3.0, |v| v.parse().expect("speed"));
    let quality_samples: usize = opt("quality").map_or(0, |v| v.parse().expect("quality"));
    let loop_path: Option<[f32; 3]> = opt("loop").map(|v| {
        let p: Vec<f32> = v
            .split(',')
            .map(|n| n.parse().expect("loop=x,z,radius"))
            .collect();
        [p[0], p[1], p[2]]
    });

    let lod_bytes = std::fs::read(dir.join("lod-meta.json")).expect("lod-meta.json");
    let meta = LodMeta::parse(&lod_bytes).expect("parse lod-meta");
    let chunk_metas: Vec<Option<Vec<u8>>> = meta
        .filenames
        .iter()
        .map(|name| std::fs::read(dir.join(name)).ok())
        .collect();
    let hint = std::fs::read(dir.join("viewer-settings.json"))
        .ok()
        .and_then(|b| CameraHint::from_json(&b).ok());
    let chunks = meta.filenames.len();
    let lod0_total = meta.counts.first().copied().unwrap_or(0);
    let mut settings = browser_settings(capacity, chunks);
    if let Some(overrides) = opt("set") {
        apply_overrides(&mut settings, overrides);
    }
    println!("settings: {settings:?}");
    let mut stream = LevelStream::new(meta, &chunk_metas, settings).expect("create stream");

    let (device, queue) = countersplat_render::try_device().expect("no GPU adapter");
    let pipelines = Pipelines::new(&device, COLOR_FORMAT).expect("pipelines");
    let rows = stream.palette_texture_rows(device.limits().max_texture_dimension_2d);
    // Real GPU memory, from the allocator, rather than an estimate from buffer sizes.
    let allocated = |device: &wgpu::Device| {
        device
            .generate_allocator_report()
            .map_or(0, |report| report.total_allocated_bytes)
    };
    let before_pool = allocated(&device);
    let pool = scene::pool_for_level(
        &device,
        &queue,
        &pipelines,
        stream.shared_meta(),
        capacity,
        0.5,
        rows,
    )
    .expect("pool");
    stream.init_pool(&pool, &queue);
    let pool_bytes = allocated(&device).saturating_sub(before_pool);
    println!(
        "pool GPU memory: {:.1} MiB for {capacity} splats = {:.1} bytes a splat (allocator-reported)",
        pool_bytes as f64 / 1_048_576.0,
        pool_bytes as f64 / f64::from(capacity)
    );
    if opt("memreport").is_some() {
        if let Some(report) = device.generate_allocator_report() {
            let mut allocations: Vec<_> = report.allocations.iter().collect();
            allocations.sort_by_key(|a| std::cmp::Reverse(a.size));
            println!(
                "allocator: {:.1} MiB allocated in {} allocations, {:.1} MiB reserved in {} blocks",
                report.total_allocated_bytes as f64 / 1_048_576.0,
                allocations.len(),
                report.total_reserved_bytes as f64 / 1_048_576.0,
                report.blocks.len()
            );
            for a in allocations.iter().take(24) {
                println!(
                    "  {:>9.2} MiB  {:>6.2} B/splat  {}",
                    a.size as f64 / 1_048_576.0,
                    a.size as f64 / f64::from(capacity),
                    a.name
                );
            }
        }
    }

    let extent = wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let target = |label, format| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let color = target("bench color", COLOR_FORMAT);
    let depth = target("bench depth", DEPTH_FORMAT);

    // Scans are stored y-down, and the viewer shows them through a 180-degree rotation about z (x and y flip). The
    // authored camera is in that rotated world, so bring it back to splat space; "up" there is -y, and the model
    // passed to the renderer is the identity.
    let hint = hint.expect("level has no viewer camera to start from");
    let flip = |p: [f32; 3]| [-p[0], -p[1], p[2]];
    let hint = CameraHint {
        position: flip(hint.position),
        target: flip(hint.target),
        ..hint
    };
    let mut eye = hint.position;
    let start_dir = {
        let d = [0, 1, 2].map(|i| hint.target[i] - hint.position[i]);
        let h = (d[0] * d[0] + d[2] * d[2]).sqrt().max(1e-6);
        [d[0] / h, d[2] / h]
    };
    let base_yaw = start_dir[1].atan2(start_dir[0]);
    let projection = camera::perspective(
        hint.fov_degrees.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
        0.05,
        1000.0,
    );
    // The browser host passes `projection[5] * render_height`; `focal=<ratio>` scales that to test other conventions
    // (0.5 would be the true pixel focal length, 0.75 the documented "540 at a 720-pixel view").
    let focal_ratio: f32 = opt("focal").map_or(1.0, |v| v.parse().expect("focal ratio"));
    let focal_px = projection[5] * HEIGHT as f32 * focal_ratio;
    stream.set_focal_px(focal_px);
    let mut oracle = (quality_samples > 0).then(|| {
        Oracle::new(
            &dir, &device, &queue, &pipelines, &settings, lod0_total, focal_px,
        )
    });

    println!(
        "{} | capacity {capacity} | {seconds} s at {speed} m/s | start {:?}",
        dir.display(),
        eye
    );

    let frames = (seconds / DT) as u64;
    // Quality checkpoints: evenly spaced after a 10 s warm-up, so the first load is not scored.
    let first_check = (10.0 / DT) as u64;
    let checkpoints: Vec<u64> = (0..quality_samples as u64)
        .map(|i| first_check + i * (frames.saturating_sub(first_check)) / quality_samples as u64)
        .collect();
    let mut psnr_samples: Vec<f64> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut decode_ms: Vec<f64> = Vec::new();
    let mut chunk_bytes: BTreeMap<usize, usize> = BTreeMap::new();
    let mut requested = 0u64;
    let mut uploaded_total = 0u64;
    let mut upload_spikes = 0u64;
    let mut max_upload = 0u32;
    let mut apply_ms: Vec<f64> = Vec::new();
    let mut select_ms: Vec<f64> = Vec::new();
    let mut gpu_ms: Vec<f64> = Vec::new();
    let mut resident_peak = 0u32;
    let mut resident_sum = 0f64;
    let mut chunks_loaded_peak = 0usize;
    let mut chunks_loaded_sum = 0f64;
    let mut empty_visible_frames = 0u64;
    let mut empty_sum = 0f64;
    let mut pending_sum = 0f64;
    let mut detail_sum = 0f64;
    let mut largest_free_min = u32::MAX;
    let mut fading_peak = 0u32;
    let mut forward = [0.0f32; 3];

    let eye_height = eye[1];
    for frame in 0..frames {
        let t = frame as f32 * DT;
        if let Some([cx, cz, radius]) = loop_path {
            // Around a circle, looking along the direction of travel with a gentle sway.
            let angle = speed / radius * t;
            eye = [
                cx + radius * angle.cos(),
                eye_height,
                cz + radius * angle.sin(),
            ];
            let heading = angle + std::f32::consts::FRAC_PI_2 + 0.5 * (0.5 * t).sin();
            forward = [heading.cos(), 0.0, heading.sin()];
        } else {
            // A straight walk forward at `speed`, swinging the view gently side to side.
            let yaw = base_yaw + 0.35 * (0.4 * t).sin();
            forward = [yaw.cos(), 0.0, yaw.sin()];
            for i in 0..3 {
                eye[i] += forward[i] * speed * DT;
            }
        }
        let target_point = [0, 1, 2].map(|i| eye[i] + forward[i]);
        let view_matrix = camera::look_at(eye, target_point, [0.0, -1.0, 0.0]);
        let clip = camera::mul(&projection, &view_matrix);

        let started = Instant::now();
        stream.select(eye, forward, Some(&clip), t);
        select_ms.push(started.elapsed().as_secs_f64() * 1000.0);

        // Requests start only on idle workers, as the browser does; the chunk arrives after its decode time.
        let idle = WORKERS.saturating_sub(pending.len());
        for (file, folder) in stream.requests_up_to(idle) {
            requested += 1;
            let begun = Instant::now();
            match sog::load_dir(&dir.join(&folder)) {
                Ok(bundle) => {
                    let ms = begun.elapsed().as_secs_f64() * 1000.0;
                    decode_ms.push(ms);
                    chunk_bytes.insert(file, bundle_bytes(&bundle));
                    let latency = ((ms / 1000.0 / f64::from(DT)).ceil() as u64).max(1);
                    pending.push(Pending {
                        file,
                        arrives: frame + latency,
                        bundle: Some(bundle),
                    });
                }
                Err(error) => {
                    eprintln!("{folder}: {error}");
                    stream.failed(file);
                }
            }
        }
        for p in pending.iter_mut().filter(|p| p.arrives <= frame) {
            if let Some(bundle) = p.bundle.take() {
                stream.insert(p.file, bundle);
            }
        }
        pending.retain(|p| p.bundle.is_some());

        let started = Instant::now();
        let uploaded = stream.apply(&pool, &queue);
        apply_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        uploaded_total += u64::from(uploaded);
        max_upload = max_upload.max(uploaded);
        if uploaded >= 200_000 {
            upload_spikes += 1;
        }

        let stats = stream.stats();
        resident_peak = resident_peak.max(stats.resident_splats);
        resident_sum += f64::from(stats.resident_splats);
        chunks_loaded_peak = chunks_loaded_peak.max(stats.chunks_loaded);
        chunks_loaded_sum += stats.chunks_loaded as f64;
        // Leaves in view that have nothing to draw yet are holes in the picture: the worst kind of pop-in.
        let holes = stats.leaves_empty;
        if holes > 0 {
            empty_visible_frames += 1;
        }
        empty_sum += f64::from(holes);
        pending_sum += f64::from(stats.leaves_pending);
        detail_sum += f64::from(stats.detail_met);
        largest_free_min = largest_free_min.min(stats.largest_free_range);
        fading_peak = fading_peak.max(stats.fading);

        // Score the picture the loader is showing now against the full-detail oracle for the same pose.
        if let Some(oracle) = oracle.as_mut().filter(|_| checkpoints.contains(&frame)) {
            let view = View {
                model: camera::IDENTITY,
                view: view_matrix,
                projection,
                width: WIDTH,
                height: HEIGHT,
                near: 0.05,
                far: 1000.0,
            };
            let live = pool.render_rgba(&device, &queue, &pipelines, &view, None);
            oracle.show(&dir, &queue, eye, forward, &clip);
            if opt("debug").is_some() {
                let s = oracle.stream.stats();
                println!(
                    "oracle: resident {} wanted {} free {} pending {} empty {} hidden {} visible {} in_flight {} loaded {} lods {:?}",
                    s.resident_splats, s.wanted_splats, s.free_slots, s.leaves_pending, s.leaves_empty,
                    s.leaves_hidden, s.leaves_visible, s.chunks_in_flight, s.chunks_loaded, s.leaves_per_lod
                );
            }
            let reference = oracle
                .pool
                .render_rgba(&device, &queue, &pipelines, &view, None);
            psnr_samples.push(content_mse(&live, &reference));
            if let Some(out) = opt("dump") {
                let n = psnr_samples.len();
                for (name, pixels) in [("live", &live), ("oracle", &reference)] {
                    let path = Path::new(out).join(format!("{n:02}_{name}.png"));
                    image::save_buffer(&path, pixels, WIDTH, HEIGHT, image::ColorType::Rgba8)
                        .expect("write png");
                }
            }
        }

        // Time the draw on a sample of frames (the GPU cost of what the stream selected).
        if frame % 30 == 0 {
            let view = View {
                model: camera::IDENTITY,
                view: view_matrix,
                projection,
                width: WIDTH,
                height: HEIGHT,
                near: 0.05,
                far: 1000.0,
            };
            let started = Instant::now();
            let mut encoder = device.create_command_encoder(&Default::default());
            pool.prepare(&queue, &mut encoder, &view, &view.model);
            {
                let mut pass = color_pass(&mut encoder, &color, &depth, true, true, "stream bench");
                pool.draw_color(&mut pass, &pipelines);
            }
            queue.submit(Some(encoder.finish()));
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            gpu_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    // `hold=1`: stop the walk, let the loader finish at the final pose, then time the draw alone (a still camera keeps
    // the cached sort) and the sort plus draw (the camera turns a little every frame). A stable GPU A/B for shader and
    // sort changes, which the sampled draw times above are too noisy for.
    if opt("hold").is_some() {
        let target_point = [0, 1, 2].map(|i| eye[i] + forward[i]);
        let view_matrix = camera::look_at(eye, target_point, [0.0, -1.0, 0.0]);
        let clip = camera::mul(&projection, &view_matrix);
        let time_frame = |view_matrix: &[f32; 16]| -> f64 {
            let view = View {
                model: camera::IDENTITY,
                view: *view_matrix,
                projection,
                width: WIDTH,
                height: HEIGHT,
                near: 0.05,
                far: 1000.0,
            };
            let started = Instant::now();
            let mut encoder = device.create_command_encoder(&Default::default());
            pool.prepare(&queue, &mut encoder, &view, &view.model);
            {
                let mut pass = color_pass(&mut encoder, &color, &depth, true, true, "stream bench hold");
                pool.draw_color(&mut pass, &pipelines);
            }
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            started.elapsed().as_secs_f64() * 1000.0
        };
        // Stopping: the loader keeps finishing crossfades at a still camera. Each frame draws, and the sorter's
        // counters show how many frames needed a full sort and how many only rewrote fade shares.
        pool.reset_profile();
        let mut settling = Vec::new();
        for settle in 0..400u64 {
            let t = (frames + settle) as f32 * DT;
            stream.select(eye, forward, Some(&clip), t);
            for (file, folder) in stream.requests_up_to(WORKERS) {
                match sog::load_dir(&dir.join(&folder)) {
                    Ok(bundle) => stream.insert(file, bundle),
                    Err(_) => stream.failed(file),
                }
            }
            stream.apply(&pool, &queue);
            settling.push(time_frame(&view_matrix));
        }
        let counters = pool.profile_json()["sort"]["counters"].clone();
        println!(
            "hold settle: 400 still frames, sorts {} fade-only {} cache hits {}, frame median {:.2} ms",
            counters["sorts"], counters["fade_updates"], counters["cache_hits"],
            { settling.sort_by(f64::total_cmp); settling[settling.len() / 2] }
        );        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            (v[v.len() / 2], v[v.len() / 10], v[v.len() * 9 / 10])
        };
        for _ in 0..30 {
            time_frame(&view_matrix);
        }
        let still: Vec<f64> = (0..200).map(|_| time_frame(&view_matrix)).collect();
        let turning: Vec<f64> = (0..200)
            .map(|i| {
                let yaw = (i as f32) * 0.004;
                let (s, c) = yaw.sin_cos();
                let f = [forward[0] * c - forward[2] * s, 0.0, forward[0] * s + forward[2] * c];
                let target = [0, 1, 2].map(|k| eye[k] + f[k]);
                time_frame(&camera::look_at(eye, target, [0.0, -1.0, 0.0]))
            })
            .collect();
        let (still, turning) = (median(still), median(turning));
        println!(
            "hold: resident {} drawn {} | draw only median {:.2} ms (p10 {:.2}, p90 {:.2}) | sort+draw median {:.2} ms (p10 {:.2}, p90 {:.2})",
            stream.stats().resident_splats,
            pool.drawn_splats(&device, &queue),
            still.0, still.1, still.2, turning.0, turning.1, turning.2
        );
    }

    let stats = stream.stats();
    let n = frames as f64;
    let pct = |v: &mut Vec<f64>, p: f64| -> f64 {
        v.sort_by(f64::total_cmp);
        v.get(((v.len().saturating_sub(1)) as f64 * p) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let mean_chunk_mib = if chunk_bytes.is_empty() {
        0.0
    } else {
        chunk_bytes.values().sum::<usize>() as f64 / chunk_bytes.len() as f64 / 1_048_576.0
    };
    println!("--- pop-in");
    println!(
        "LOD switches {} (near {}), hard swaps {}, fades cut {}, peak crossfades {}",
        stats.switches, stats.near_switches, stats.hard_swaps, stats.fades_cut, fading_peak
    );
    println!(
        "frames with a leaf in view and nothing to draw: {} of {} ({:.1}%), mean empty leaves {:.2}",
        empty_visible_frames,
        frames,
        100.0 * empty_visible_frames as f64 / n,
        empty_sum / n
    );
    println!(
        "mean leaves pending a different LOD {:.1}, mean detail met {:.3}",
        pending_sum / n,
        detail_sum / n
    );
    if !psnr_samples.is_empty() {
        // Pool the squared errors, then convert: an average of dB values is dominated by near-identical frames.
        let mean_mse = psnr_samples.iter().sum::<f64>() / psnr_samples.len() as f64;
        let worst_mse = psnr_samples.iter().cloned().fold(0.0, f64::max);
        println!(
            "--- quality vs full-detail oracle ({} frames, content pixels only)",
            psnr_samples.len()
        );
        println!(
            "PSNR pooled {:.2} dB, worst {:.2} dB; each frame: {}",
            psnr_of(mean_mse),
            psnr_of(worst_mse),
            psnr_samples
                .iter()
                .map(|&v| format!("{:.1}", psnr_of(v)))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    println!("--- memory");
    println!(
        "pool resident splats: peak {resident_peak}, mean {:.0} of {capacity} ({:.0}% peak)",
        resident_sum / n,
        100.0 * f64::from(resident_peak) / f64::from(capacity)
    );
    println!(
        "decoded chunks held: peak {chunks_loaded_peak}, mean {:.1}; ~{mean_chunk_mib:.1} MiB each => peak ~{:.0} MiB",
        chunks_loaded_sum / n,
        chunks_loaded_peak as f64 * mean_chunk_mib
    );
    println!(
        "smallest largest-free-range: {largest_free_min} (fragmentation floor), upload total {uploaded_total} splats"
    );
    println!("--- cost");
    println!(
        "chunks requested {requested}, decode ms p50 {:.0} p95 {:.0}, total {:.1} s",
        pct(&mut decode_ms.clone(), 0.5),
        pct(&mut decode_ms.clone(), 0.95),
        decode_ms.iter().sum::<f64>() / 1000.0
    );
    println!(
        "uploads: max {max_upload} splats in one frame, {upload_spikes} frames >= 200k splats"
    );
    println!(
        "select ms p50 {:.3} p99 {:.3} | apply ms p50 {:.3} p99 {:.3} max {:.2}",
        pct(&mut select_ms.clone(), 0.5),
        pct(&mut select_ms.clone(), 0.99),
        pct(&mut apply_ms.clone(), 0.5),
        pct(&mut apply_ms.clone(), 0.99),
        pct(&mut apply_ms.clone(), 1.0)
    );
    println!(
        "draw GPU ms (sampled) p50 {:.2} p90 {:.2} max {:.2}",
        pct(&mut gpu_ms.clone(), 0.5),
        pct(&mut gpu_ms.clone(), 0.9),
        pct(&mut gpu_ms.clone(), 1.0)
    );
    let _ = (&forward, Path::new(""));
}
