//! Turns a green-screen still into a first-person footage frame: resized to the footage resolution, with the
//! near-green backdrop snapped to exact chroma green so the weapon key is clean, written as lossless WebP.
//!
//! `shift=<fraction>` moves the picture down by that fraction of the frame height (filling the top with green),
//! to derive a lower-held pose from a raised one.
//!
//! ```text
//! cargo run --release -p countersplat-assets --example footage_still -- in.png out.webp [width height] [shift=0.18]
//! ```
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, Rgb, RgbImage};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let shift = match args.iter().position(|a| a.starts_with("shift=")) {
        Some(i) => args.remove(i)["shift=".len()..].parse::<f32>().expect("shift fraction"),
        None => 0.0,
    };
    assert!(args.len() == 2 || args.len() == 4, "usage: in.png out.webp [width height] [shift=fraction]");
    assert!((0.0..0.9).contains(&shift), "shift must be 0..0.9");
    let (width, height) = match args.get(2) {
        Some(w) => (w.parse().unwrap(), args[3].parse().unwrap()),
        None => (1280, 720),
    };
    let resized = image::open(&args[0])
        .expect("read the still")
        .resize_exact(width, height, FilterType::Lanczos3)
        .to_rgb8();
    let offset = (shift * height as f32).round() as u32;
    let mut frame = RgbImage::from_pixel(width, height, Rgb([0, 255, 0]));
    for y in offset..height {
        for x in 0..width {
            frame.put_pixel(x, y, *resized.get_pixel(x, y - offset));
        }
    }
    let mut snapped = 0usize;
    for Rgb([r, g, b]) in frame.pixels_mut() {
        // Backdrop pixels: strongly green and far from any grey or skin tone. Edges keep their blend.
        if *g >= 200 && *r <= 90 && *b <= 90 {
            (*r, *g, *b) = (0, 255, 0);
            snapped += 1;
        }
    }
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(frame.as_raw(), width, height, ExtendedColorType::Rgb8)
        .expect("encode WebP");
    std::fs::write(&args[1], &out).expect("write the frame");
    let total = (width * height) as f64;
    println!("{} {width}x{height} {} KiB, {:.0}% backdrop", args[1], out.len() / 1024, snapped as f64 / total * 100.0);
}
