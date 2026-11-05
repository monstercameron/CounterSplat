//! Renders a level with the sort's footprint cull off and on (`sort::set_footprint_cull`) from several views and
//! checks that every byte matches, printing how many splats the cull left out. With an output directory it also
//! writes each culled frame as a PNG to look at.
//!
//! ```text
//! cargo run --release -p countersplat-render --example cull_verify -- assets/levels/<id> [width] [height] [png dir]
//! ```
use countersplat_render::{
    camera,
    scene::{self, Pipelines, View},
    sort, COLOR_FORMAT,
};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("level directory"));
    let width: u32 = args.next().map_or(1421, |v| v.parse().expect("width"));
    let height: u32 = args.next().map_or(1266, |v| v.parse().expect("height"));
    let out = args.next().map(PathBuf::from);
    let (device, queue) = countersplat_render::try_device().expect("no GPU adapter");
    let pipelines = Pipelines::new(&device, COLOR_FORMAT).expect("pipelines");
    let bundle = countersplat_assets::sog::load_dir(&dir).expect("load level");
    let scene = scene::build(&device, &queue, &pipelines, &bundle, 0.5).expect("build scene");
    // The same level sorted the old way, every slot with no compaction (so no early-out radix workgroups either):
    // an independent reference for the compacting sort on a real level.
    scene::set_indirect_draw(false);
    let reference = scene::build(&device, &queue, &pipelines, &bundle, 0.5).expect("build reference scene");
    scene::set_indirect_draw(true);
    let mut failures = 0;
    // (label, eye, yaw degrees, camera effects: exposure EV, focus distance, blur scale, shutter)
    let cases = [
        ("spawn", [0.0, 0.0, 0.0], 0.0, [-0.03, 7.5, 0.0, 0.0]),
        ("yaw 90", [0.0, 0.0, 0.0], 90.0, [-0.03, 7.5, 0.0, 0.0]),
        ("yaw 180", [0.0, 0.0, 0.0], 180.0, [-0.03, 7.5, 0.0, 0.0]),
        ("yaw 270", [0.0, 0.0, 0.0], 270.0, [-0.03, 7.5, 0.0, 0.0]),
        ("moved, yaw 45", [1.5, 0.3, -2.0], 45.0, [-0.03, 7.5, 0.0, 0.0]),
        ("yaw 90, focus blur and motion blur", [0.0, 0.0, 0.0], 90.0, [-0.03, 2.0, 8.0, 0.5]),
    ];
    for (index, (label, eye, yaw_deg, effects)) in cases.into_iter().enumerate() {
        let yaw = f32::to_radians(yaw_deg);
        let target = [eye[0] + yaw.sin() * 2.0, eye[1], eye[2] - yaw.cos() * 2.0];
        let view = View {
            model: camera::IDENTITY,
            view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
            projection: camera::perspective(75f32.to_radians(), width as f32 / height as f32, 0.05, 1000.0),
            width,
            height,
            near: 0.05,
            far: 1000.0,
        };
        // Motion blur smears along the change from a slightly turned previous view.
        let previous_target = [eye[0] + (yaw - 0.03).sin() * 2.0, eye[1], eye[2] - (yaw - 0.03).cos() * 2.0];
        let previous = camera::mul(&view.projection, &camera::look_at(eye, previous_target, [0.0, 1.0, 0.0]));
        scene.set_camera_effects(&queue, effects, &previous);
        sort::set_footprint_cull(false);
        let full = scene.render_rgba(&device, &queue, &pipelines, &view, None);
        let all = scene.drawn_splats(&device, &queue);
        sort::set_footprint_cull(true);
        let culled = scene.render_rgba(&device, &queue, &pipelines, &view, None);
        let kept = scene.drawn_splats(&device, &queue);
        let differing = full.iter().zip(&culled).filter(|(a, b)| a != b).count();
        reference.set_camera_effects(&queue, effects, &previous);
        let uncompacted = reference.render_rgba(&device, &queue, &pipelines, &view, None);
        let against_reference = uncompacted.iter().zip(&culled).filter(|(a, b)| a != b).count();
        if differing != 0 || against_reference != 0 {
            failures += 1;
        }
        println!(
            "{label:<38} drawn {all:>8} -> {kept:>8} ({:>5.1}% left out), differing bytes {differing}, vs full sort {against_reference}",
            100.0 * (all - kept) as f64 / all.max(1) as f64
        );
        if let Some(out) = &out {
            std::fs::create_dir_all(out).expect("output directory");
            image::save_buffer(out.join(format!("cull_{index}.png")), &culled, width, height, image::ColorType::Rgba8)
                .expect("write png");
        }
    }
    assert_eq!(failures, 0, "the footprint cull changed {failures} frames");
}
