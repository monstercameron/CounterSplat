//! Estimates how many splats that pass the sort's coarse view cull are then discarded by the vertex stage anyway
//! (projected size under `minPixelSize`, or opacity at or under the forward alpha clip), at the camera paths of
//! `splat_bench`. The size test uses an upper bound on the projected footprint (largest axis, Jacobian norm), so the
//! count is a lower bound on what the vertex stage discards.
//!
//! ```text
//! cargo run --release -p countersplat-render --example cull_estimate -- assets/levels/<id> [width] [height]
//! ```
use countersplat_render::camera;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("level directory"));
    let width: f32 = args.next().map_or(1280.0, |v| v.parse().expect("width"));
    let height: f32 = args.next().map_or(720.0, |v| v.parse().expect("height"));
    let bundle = countersplat_assets::sog::load_dir(&dir).expect("load level");
    let centres = countersplat_assets::sog::centres(&bundle);
    let scales = &bundle.textures["scales"].rgba;
    let sh0 = &bundle.textures["sh0"].rgba;
    let codebook = &bundle.meta.scales_codebook;
    let max_scale: Vec<f32> = (0..centres.len())
        .map(|i| (0..3).map(|c| codebook[usize::from(scales[i * 4 + c])].exp()).fold(0.0, f32::max))
        .collect();
    let faint = (0..centres.len()).filter(|&i| sh0[i * 4 + 3] <= 1).count();
    println!("{} splats; opacity byte <= 1 (always discarded): {faint}", centres.len());

    for yaw_deg in [0.0f32, 90.0, 180.0] {
        let yaw = yaw_deg.to_radians();
        let view = camera::look_at([0.0; 3], [yaw.sin() * 2.0, 0.0, -yaw.cos() * 2.0], [0.0, 1.0, 0.0]);
        let projection = camera::perspective(75f32.to_radians(), width / height, 0.05, 1000.0);
        let clip = camera::mul(&projection, &view);
        let (fx, fy) = (width * projection[0], height * projection[5]);
        let focal = fx.abs().max(fy.abs());
        let (mut in_view, mut small, mut faint_in_view) = (0usize, 0usize, 0usize);
        let (mut offscreen, mut drawn_bound) = (0usize, 0usize);
        let mut by_bound = [0usize; 4];
        for (i, c) in centres.iter().enumerate() {
            let p = |m: &[f32; 16], r: usize| m[r] * c[0] + m[4 + r] * c[1] + m[8 + r] * c[2] + m[12 + r];
            let (x, y, w) = (p(&clip, 0), p(&clip, 1), p(&clip, 3));
            if w <= 0.0 || x.abs() > w * 1.5 || y.abs() > w * 1.5 {
                continue;
            }
            in_view += 1;
            if sh0[i * 4 + 3] <= 1 {
                faint_in_view += 1;
                continue;
            }
            let (vx, vy, vz) = (p(&view, 0), p(&view, 1), -p(&view, 2));
            let jacobian2 = (focal / vz).powi(2) * (1.0 + (vx * vx + vy * vy) / (vz * vz));
            let lambda = max_scale[i].powi(2) * jacobian2;
            if lambda < 0.2 {
                small += 1;
            }
            // The vertex stage's frustum test with the footprint bound: off screen even at its largest extent.
            let l = 2.0 * (2.0 * (lambda + 0.3)).sqrt();
            if x.abs() - l * w / width > w || y.abs() - l * w / height > w {
                offscreen += 1;
            } else if lambda >= 0.2 {
                drawn_bound += 1;
            }
            // How far under the threshold: a histogram of the footprint bound against the cut.
            let bucket = match lambda / 0.2 {
                r if r < 0.25 => 0,
                r if r < 1.0 => 1,
                r if r < 4.0 => 2,
                _ => 3,
            };
            by_bound[bucket] += 1;
        }
        println!(
            "yaw {yaw_deg:>5}: in view {in_view}, faint {faint_in_view} ({:.1}%), surely under minPixelSize {small} ({:.1}%); bound/cut <0.25 {} | <1 {} | <4 {} | >=4 {}",
            100.0 * faint_in_view as f64 / in_view.max(1) as f64,
            100.0 * small as f64 / in_view.max(1) as f64,
            by_bound[0], by_bound[1], by_bound[2], by_bound[3]
        );
        println!(
            "            surely off screen {offscreen} ({:.1}%); at most {drawn_bound} ({:.1}%) can reach the screen",
            100.0 * offscreen as f64 / in_view.max(1) as f64,
            100.0 * drawn_bound as f64 / in_view.max(1) as f64
        );
    }
}
