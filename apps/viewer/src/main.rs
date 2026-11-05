//! Small native host for exercising the same public APIs a game or editor would use.
//!
//! `inspect` needs no GPU. `render` draws one SOG asset; `render-level` loads requested LOD chunks into a GPU pool.
//! The program deliberately contains no gameplay or engine-internal file access.

use countersplat_assets::{lod::LodMeta, sog};
use countersplat_render::{
    camera,
    scene::{self, Pipelines, View},
    stream::{LevelStream, StreamSettings},
};
use std::{error::Error, path::Path, time::Duration};

mod threaded;
mod window;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;

fn main() {
    if let Err(error) = run() {
        eprintln!("countersplat-viewer: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, input, options @ ..] if command == "window-level" => {
            window::run(Path::new(input), options)
        }
        [command, input] if command == "inspect" => {
            threaded::inspect_sog(Path::new(input))
        }
        [command, input] if command == "inspect-level" => {
            threaded::inspect_level(Path::new(input))
        }
        [command, input, output] if command == "render" => threaded::render_sog(Path::new(input), Path::new(output)),
        [command, input, output, seconds, options @ ..] if command == "live-level" => {
            let seconds: f64 = seconds.parse()?;
            let duration = Duration::try_from_secs_f64(seconds)?;
            if duration.is_zero() { return Err("duration must be greater than zero".into()); }
            let (capacity, rotate) = match options {
                [] => (500_000, false),
                [capacity] => (capacity.parse::<u32>()?, false),
                [capacity, orientation] if orientation == "z180" => (capacity.parse::<u32>()?, true),
                _ => return Err("usage: live-level <level-dir> <out.png> <seconds> [pool-splats] [z180]".into()),
            };
            if capacity == 0 { return Err("capacity must be greater than zero".into()); }
            threaded::live_level(Path::new(input), Path::new(output), capacity, rotate, duration)
        }
        [command, input, output] if command == "render-level" => threaded::render_level(Path::new(input), Path::new(output), 500_000, false),
        [command, input, output, capacity] if command == "render-level" => {
            let capacity: u32 = capacity.parse()?;
            if capacity == 0 { return Err("capacity must be greater than zero".into()); }
            threaded::render_level(Path::new(input), Path::new(output), capacity, false)
        }
        [command, input, output, capacity, orientation] if command == "render-level" && orientation == "z180" => {
            let capacity: u32 = capacity.parse()?;
            if capacity == 0 { return Err("capacity must be greater than zero".into()); }
            threaded::render_level(Path::new(input), Path::new(output), capacity, true)
        }
        _ => Err("usage: countersplat-viewer window-level <level-dir> [--z180] [--capacity=N] [--vsync=on|off] [--fps=N|monitor|unlimited] [--scene=FILE] [--sky=FILE|--no-sky] [--sky-exposure=N] [--sky-rotation=DEG] | inspect <file.sog|chunk-dir> | inspect-level <level-dir> | render <file.sog|chunk-dir> <out.png> | render-level <level-dir> <out.png> [pool-splats] [z180] | live-level <level-dir> <out.png> <seconds> [pool-splats] [z180]".into()),
    }
}

fn load_sog(path: &Path) -> Result<sog::Bundle> {
    if path.is_dir() {
        sog::load_dir(path)
    } else {
        sog::load(path)
    }
}

fn load_lod(dir: &Path) -> Result<LodMeta> {
    LodMeta::parse(&std::fs::read(dir.join("lod-meta.json"))?).map_err(Into::into)
}

fn device() -> Result<(wgpu::Device, wgpu::Queue)> {
    countersplat_render::try_device().ok_or_else(|| "no compatible GPU device is available".into())
}

/// Select a useful initial framing without encoding application-specific spawn or camera rules.
fn view_for_bounds(min: [f32; 3], max: [f32; 3]) -> (View, [f32; 3], [f32; 3]) {
    let center = [0, 1, 2].map(|axis| (min[axis] + max[axis]) * 0.5);
    let size = (0..3)
        .map(|axis| (max[axis] - min[axis]).abs())
        .fold(0.0f32, f32::max)
        .max(1.0);
    let eye = [center[0], center[1] + size * 0.2, center[2] + size * 1.8];
    let direction = [center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]];
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at(eye, center, [0.0, 1.0, 0.0]),
        projection: camera::perspective(
            60f32.to_radians(),
            WIDTH as f32 / HEIGHT as f32,
            0.05,
            size * 20.0 + 100.0,
        ),
        width: WIDTH,
        height: HEIGHT,
        near: 0.05,
        far: size * 20.0 + 100.0,
    };
    (view, eye, direction)
}

fn save_image(
    scene: &scene::GsplatScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    view: &View,
    output: &Path,
) -> Result<()> {
    write_image(scene, device, queue, pipelines, view, output)?;
    println!("Rendered {}", output.display());
    Ok(())
}

fn write_image(
    scene: &scene::GsplatScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    view: &View,
    output: &Path,
) -> Result<()> {
    let pixels = scene.render_rgba(device, queue, pipelines, view, None);
    let image = image::RgbaImage::from_raw(WIDTH, HEIGHT, pixels).ok_or("unexpected image size")?;
    // Finish encoding before replacing the file contents, so local preview readers see the old frame
    // for most of the work instead of an incompletely encoded PNG.
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    std::fs::write(output, png.into_inner())?;
    Ok(())
}

fn render_sog(
    bundle: sog::Bundle,
    output: &Path,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
) -> Result<()> {
    if bundle.meta.count == 0 {
        return Err("SOG asset contains no splats".into());
    }
    let scene = scene::build(device, queue, pipelines, &bundle, 0.5)?;
    let (view, eye, _) = view_for_bounds(scene.bounds.0, scene.bounds.1);
    println!("Scene bounds {:?}; camera {:?}", scene.bounds, eye);
    save_image(&scene, device, queue, pipelines, &view, output)
}

struct LoadedLevel {
    pool: scene::GsplatScene,
    view: View,
    eye: [f32; 3],
    direction: [f32; 3],
    up: [f32; 3],
}

fn render_level(
    data: threaded::LevelData,
    input: &Path,
    output: &Path,
    capacity: u32,
    rotate_z_180: bool,
    assets: &threaded::AssetClient,
    gpu: &threaded::Gpu,
) -> Result<()> {
    let level = prepare_level(data, input, capacity, rotate_z_180, assets, gpu)?;
    save_image(
        &level.pool,
        &gpu.device,
        &gpu.queue,
        &gpu.pipelines,
        &level.view,
        output,
    )
}

fn prepare_level(
    data: threaded::LevelData,
    input: &Path,
    capacity: u32,
    rotate_z_180: bool,
    assets: &threaded::AssetClient,
    gpu: &threaded::Gpu,
) -> Result<LoadedLevel> {
    let device = &gpu.device;
    let queue = &gpu.queue;
    let pipelines = &gpu.pipelines;
    let threaded::LevelData { meta, metas, hint } = data;
    let has_hint = hint.is_some();
    let first = meta.leaves.first().ok_or("level has no leaves")?;
    let (mut min, mut max) = (first.min, first.max);
    for leaf in &meta.leaves {
        for axis in 0..3 {
            min[axis] = min[axis].min(leaf.min[axis]);
            max[axis] = max[axis].max(leaf.max[axis]);
        }
    }
    let size = (0..3)
        .map(|axis| (max[axis] - min[axis]).abs())
        .fold(0.0f32, f32::max)
        .max(1.0);
    let settings = StreamSettings {
        capacity,
        max_distance_m: (size * 4.0).max(150.0),
        ..Default::default()
    };
    let mut stream = LevelStream::new(meta, &metas, settings)?;
    let rows = stream.palette_texture_rows(device.limits().max_texture_dimension_2d);
    let pool = scene::pool_for_level(
        device,
        queue,
        pipelines,
        stream.shared_meta(),
        capacity,
        0.5,
        rows,
    )?;
    stream.init_pool(&pool, queue);
    // Scan metadata and authored cameras can use different up-axis conventions. Keep the choice at the host
    // boundary; the renderer and streamer always receive the same model transform and matching local-space camera.
    let model = if rotate_z_180 {
        [
            -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    } else {
        camera::IDENTITY
    };
    let (view, eye, direction) = if let Some(hint) = hint {
        let inverse = camera::inverse(&model).ok_or("invalid scan transform")?;
        let eye = camera::transform_point(&inverse, hint.position);
        let target = camera::transform_point(&inverse, hint.target);
        let direction = [0, 1, 2].map(|axis| target[axis] - eye[axis]);
        let view = View {
            model,
            view: camera::look_at(hint.position, hint.target, [0.0, 1.0, 0.0]),
            projection: camera::perspective(
                hint.fov_degrees.to_radians(),
                WIDTH as f32 / HEIGHT as f32,
                0.05,
                size * 20.0 + 100.0,
            ),
            width: WIDTH,
            height: HEIGHT,
            near: 0.05,
            far: size * 20.0 + 100.0,
        };
        (view, eye, direction)
    } else {
        let (mut view, eye, direction) = view_for_bounds(min, max);
        let world_eye = camera::transform_point(&model, eye);
        let world_target =
            camera::transform_point(&model, [0, 1, 2].map(|i| eye[i] + direction[i]));
        view.view = camera::look_at(
            world_eye,
            world_target,
            if rotate_z_180 {
                [0.0, -1.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            },
        );
        view.model = model;
        (view, eye, direction)
    };
    stream.set_focal_px(view.projection[5] * 540.0);
    let clip = camera::mul(&view.projection, &camera::mul(&view.view, &view.model));
    let mut failed = 0usize;
    for step in 0..240 {
        stream.select(eye, direction, Some(&clip), step as f32 / 60.0);
        let requests = stream.requests();
        for (file, folder) in &requests {
            match threaded::load_chunk(assets, input.join(folder)) {
                Ok(bundle) => stream.insert(*file, bundle),
                Err(error) => {
                    eprintln!("{}: {error}", folder);
                    stream.failed(*file);
                    failed += 1;
                }
            }
        }
        stream.apply(&pool, queue);
        let stats = stream.stats();
        if requests.is_empty() && stats.leaves_pending == 0 && stats.chunks_in_flight == 0 {
            break;
        }
    }
    let stats = stream.stats();
    if stats.resident_splats == 0 {
        return Err(format!("no splats became resident ({failed} failed chunk loads)").into());
    }
    println!(
        "Loaded {} resident splats ({} failed chunk loads)",
        stats.resident_splats, failed
    );
    let world_eye = camera::transform_point(&model, eye);
    let world_target =
        camera::transform_point(&model, [0, 1, 2].map(|axis| eye[axis] + direction[axis]));
    let world_direction = [0, 1, 2].map(|axis| world_target[axis] - world_eye[axis]);
    let up = if has_hint || !rotate_z_180 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, -1.0, 0.0]
    };
    Ok(LoadedLevel {
        pool,
        view,
        eye: world_eye,
        direction: world_direction,
        up,
    })
}

/// Draw a gently moving camera driven by simulation time, retaining the scene on the GPU owner.
fn draw_live_level(
    level: &LoadedLevel,
    gpu: &threaded::Gpu,
    output: &Path,
    simulation_seconds: f64,
) -> Result<()> {
    let angle = ((simulation_seconds * 0.6).sin() * 0.08) as f32;
    let (sin, cos) = angle.sin_cos();
    let direction = level.direction;
    let target = [
        level.eye[0] + direction[0] * cos - direction[2] * sin,
        level.eye[1] + direction[1],
        level.eye[2] + direction[0] * sin + direction[2] * cos,
    ];
    let mut view = level.view;
    view.view = camera::look_at(level.eye, target, level.up);
    write_image(
        &level.pool,
        &gpu.device,
        &gpu.queue,
        &gpu.pipelines,
        &view,
        output,
    )
}
