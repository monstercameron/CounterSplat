//! Exercise the real GPU sky pass, including runtime rotation and exposure.
use countersplat_render::{
    camera,
    env::{Cubemap, Sky},
    scene,
};

fn render_center(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sky: &Sky,
    rotation: f32,
    exposure: f32,
) -> [u8; 4] {
    let size = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky test color"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: countersplat_render::COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky test depth"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: countersplat_render::DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sky readback"),
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let mut pass = scene::color_pass(
            &mut encoder,
            &color_view,
            &depth_view,
            true,
            true,
            "sky test",
        );
        let view = camera::look_at([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        let projection = camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0);
        sky.draw_rotated(queue, &mut pass, &view, &projection, exposure, rotation);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        size,
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let offset = (32 * 64 + 32) * 4;
    mapped[offset..offset + 4].try_into().unwrap()
}

#[test]
fn sky_renders_hdr_radiance_and_rotates_without_reupload() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter available; skipping skybox test");
        return;
    };
    let cube = Cubemap {
        size: 4,
        faces: std::array::from_fn(|face| {
            let color = match face {
                0 => [4.0, 0.0, 0.0],
                5 => [0.0, 0.0, 4.0],
                _ => [0.0, 0.0, 0.0],
            };
            vec![color; 16]
        }),
    };
    let sky = Sky::try_new(&device, &queue, &cube, countersplat_render::COLOR_FORMAT).unwrap();
    let blue = render_center(&device, &queue, &sky, 0.0, 1.0);
    let red = render_center(&device, &queue, &sky, 90.0, 1.0);
    let dim = render_center(&device, &queue, &sky, 90.0, 0.05);
    assert!(blue[2] > 200 && blue[0] < 30, "unrotated -Z face: {blue:?}");
    assert!(red[0] > 200 && red[2] < 30, "rotated +X face: {red:?}");
    assert!(
        dim[0] + 30 < red[0],
        "exposure must affect sky brightness: {dim:?} vs {red:?}"
    );
    assert!(Sky::from_rgba16f(&device, &queue, 4, &[], countersplat_render::COLOR_FORMAT).is_err());
}
