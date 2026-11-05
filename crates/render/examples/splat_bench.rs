//! Offscreen, GPU-saturated benchmark of the Gaussian-splat scene draw.
//!
//! The browser preview is capped at the display's refresh rate, which lets the GPU drop its clocks between
//! frames and makes small renderer changes unmeasurable. This renders the same scene back to back with no
//! presentation, waiting for the GPU after every frame, so the clocks stay up and many stable samples are cheap.
//!
//! ```text
//! cargo run --release -p countersplat-render --example splat_bench -- assets/levels/<id> [frames] [width] [height]
//! ```
//!
//! It reports the per-frame time (sort, if the camera moved, plus the draw) for three camera paths: a static
//! view, a continuous spin that re-sorts every frame, and a view turned away from the room where most splats are
//! culled. Run it before and after a renderer change on the same machine.
use countersplat_render::{
    camera,
    gpu_timing::{GpuFrameTimer, GpuTiming},
    scene::{self, color_pass, Pipelines, View},
    sort, COLOR_FORMAT, DEPTH_FORMAT,
};
use std::{path::PathBuf, time::Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("level directory (assets/levels/<id>)"));
    let frames: usize = args.next().map_or(300, |v| v.parse().expect("frame count"));
    let width: u32 = args.next().map_or(1280, |v| v.parse().expect("width"));
    let height: u32 = args.next().map_or(720, |v| v.parse().expect("height"));
    // `depth` also runs the impact-decal depth prepass (a second full splat draw) every frame, as the browser host
    // does while any bullet mark or smoke exists.
    let with_depth_pass = args.next().as_deref() == Some("depth");

    // COUNTERSPLAT_BENCH_SCENE_FORMAT=1 draws into the browser's HDR scene target (postprocess::SCENE_FORMAT,
    // Rgba16Float) instead of the sRGB output format.
    let color_format = if std::env::var("COUNTERSPLAT_BENCH_SCENE_FORMAT").is_ok_and(|v| v == "1") {
        countersplat_render::postprocess::SCENE_FORMAT
    } else {
        COLOR_FORMAT
    };
    let (device, queue) = countersplat_render::try_device().expect("no GPU adapter");
    let pipelines = Pipelines::new(&device, color_format).expect("pipelines");
    let bundle = countersplat_assets::sog::load_dir(&dir).expect("load level");
    let scene = scene::build(&device, &queue, &pipelines, &bundle, 0.5).expect("build scene");
    // `COUNTERSPLAT_BENCH_EXPOSURE_EV=-0.03` matches gameplay's camera lens, which takes the vertex stage's exposure
    // path (linearise, scale, re-encode) on every splat.
    if let Some(ev) = std::env::var("COUNTERSPLAT_BENCH_EXPOSURE_EV")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        scene.set_camera_effects(&queue, [ev, 7.5, 0.0, 0.0], &camera::IDENTITY);
    }
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let target = |label, format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let color = target(
        "bench color",
        color_format,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let depth = target(
        "bench depth",
        DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );

    println!(
        "{} splats, {width}x{height}, {frames} frames per path",
        scene.capacity()
    );
    // (label, yaw at frame 0 in degrees, degrees per frame, wobble). A wobbling path alternates the yaw by 0.05
    // degrees, so it re-sorts every frame while drawing the same view: its time minus the static path's at the same
    // yaw is the sort's cost.
    let paths = [
        ("static (spawn view)", 0.0f32, 0.0f32, false),
        ("wobble (spawn view, re-sorts)", 0.0, 0.0, true),
        ("spin (re-sorts every frame)", 0.0, 1.2, false),
        ("turned away 180 deg", 180.0, 0.0, false),
        ("turned away 90 deg", 90.0, 0.0, false),
        ("wobble 90 deg (re-sorts)", 90.0, 0.0, true),
    ];
    let only = std::env::var("COUNTERSPLAT_BENCH_PATHS").ok();
    for (label, yaw0, step, wobble) in paths {
        if only.as_deref().is_some_and(|only| !only.split(',').any(|p| label.starts_with(p))) {
            continue;
        }
        let mut times = Vec::with_capacity(frames);
        let mut spans: Vec<(&'static str, Vec<f64>)> = Vec::new();
        for frame in 0..frames + 30 {
            let jitter = if wobble && frame % 2 == 1 { 0.05 } else { 0.0 };
            let yaw = (yaw0 + step * frame as f32 + jitter).to_radians();
            let eye = [0.0, 0.0, 0.0];
            let target_point = [yaw.sin() * 2.0, 0.0, -yaw.cos() * 2.0];
            let view = View {
                model: camera::IDENTITY,
                view: camera::look_at(eye, target_point, [0.0, 1.0, 0.0]),
                projection: camera::perspective(
                    75f32.to_radians(),
                    width as f32 / height as f32,
                    0.05,
                    1000.0,
                ),
                width,
                height,
                near: 0.05,
                far: 1000.0,
            };
            let start = Instant::now();
            let mut encoder = device.create_command_encoder(&Default::default());
            // With COUNTERSPLAT_TIMESTAMPS=1 (and a GPU with timestamp queries) each frame is split into GPU spans:
            // the sort's stages (see `sort::set_stage_timer`), then the depth prepass and the colour draw.
            let timer = GpuFrameTimer::new(&device).map(std::rc::Rc::new);
            if let Some(timer) = &timer {
                timer.begin(&mut encoder, frame as u64);
                sort::set_stage_timer(Some(timer.clone()));
            }
            scene.prepare(&queue, &mut encoder, &view, &view.model);
            sort::set_stage_timer(None);
            if let Some(timer) = &timer {
                timer.mark(&mut encoder, "prepare rest");
            }
            if with_depth_pass {
                // `COUNTERSPLAT_BENCH_DEPTH_FAR=<metres>` limits the prepass as the browser does for impacts that far
                // away (the farthest impact's depth); unset, the prepass covers every splat in view.
                let far = std::env::var("COUNTERSPLAT_BENCH_DEPTH_FAR")
                    .ok()
                    .and_then(|v| v.parse::<f32>().ok());
                scene.prepare_depth_range(&queue, &mut encoder, &view, &view.model, far);
                let mut pass = scene::depth_pass(&mut encoder, &depth, "bench depth prepass");
                scene.draw_depth(&mut pass, &pipelines);
            }
            if let (Some(timer), true) = (&timer, with_depth_pass) {
                timer.mark(&mut encoder, "depth prepass");
            }
            {
                let mut pass = color_pass(
                    &mut encoder,
                    &color,
                    &depth,
                    true,
                    !with_depth_pass,
                    "bench",
                );
                scene.draw_color(&mut pass, &pipelines);
            }
            if let Some(timer) = &timer {
                timer.end(&mut encoder, "color draw");
            }
            queue.submit(Some(encoder.finish()));
            if let Some(timer) = &timer {
                timer.submitted(&queue);
            }
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            if frame >= 30 {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                if let Some(GpuTiming::Complete { spans: frame_spans, .. }) = timer.map(|timer| timer.timing()) {
                    for (name, ms) in frame_spans {
                        match spans.iter_mut().find(|(n, _)| *n == name) {
                            Some((_, list)) => list.push(ms),
                            None => spans.push((name, vec![ms])),
                        }
                    }
                }
            }
        }
        let drawn = scene.drawn_splats(&device, &queue);
        times.sort_by(f64::total_cmp);
        let at = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        println!(
            "{label:<30} median {:6.2} ms   p10 {:6.2}   p90 {:6.2}   mean {:6.2}   drawn {drawn}",
            at(0.5),
            at(0.1),
            at(0.9),
            mean
        );
        if !spans.is_empty() {
            let medians: Vec<String> = spans
                .iter_mut()
                .map(|(name, list)| {
                    list.sort_by(f64::total_cmp);
                    // A span absent from some frames (an unsorted static frame) is reported over the frames it ran in.
                    format!("{name} {:.2}", list[list.len() / 2])
                })
                .collect();
            println!("{:<30} GPU medians (ms): {}", "", medians.join(" | "));
        }
    }
}
