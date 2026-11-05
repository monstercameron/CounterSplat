//! Solid props draw where they are, lit, and write depth so later (splat) draws behind them are hidden.
use countersplat_render::{
    camera,
    props::{PropInstance, PropRenderer, MAX_PROP_INSTANCES},
    tracer::TracerRenderer,
    COLOR_FORMAT, DEPTH_FORMAT,
};

const SIZE: u32 = 64;

fn render(props: &[PropInstance], line_behind: bool) -> Option<Vec<u8>> {
    let (device, queue) = countersplat_render::try_device()?;
    let mut renderer = PropRenderer::new(&device, COLOR_FORMAT);
    renderer.set(&queue, props).unwrap();
    let mut tracer = TracerRenderer::new(&device, COLOR_FORMAT);
    if line_behind {
        // A bright line across the middle of the view, 1 unit behind the props.
        tracer.set_colored(&queue, &[-3.0, 0.0, -1.0, 3.0, 0.0, -1.0, 0.0, 0.0, 8.0, 1.0]).unwrap();
    }
    let target = |format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
    };
    let color = target(COLOR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth = target(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let view = camera::look_at([0.0, 0.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]);
    let projection = camera::perspective(60f32.to_radians(), 1.0, 0.05, 100.0);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (SIZE * 256) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.draw(&queue, &mut pass, &view, &projection, 1.0);
        tracer.draw(&queue, &mut pass, &view, &projection);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(SIZE) },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap().to_vec();
    Some(data)
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = (y * 256 + x * 4) as usize;
    [data[at], data[at + 1], data[at + 2], data[at + 3]]
}

fn red_box() -> PropInstance {
    PropInstance {
        cylinder: false,
        position: [-0.6, 0.0, 0.0],
        rotation: [0.0, (0.4f32).sin(), 0.0, (0.4f32).cos()],
        scale: [0.4, 0.5, 0.4],
        color: [0.9, 0.05, 0.05],
        cap_color: [0.9, 0.05, 0.05],
    }
}

fn green_can() -> PropInstance {
    PropInstance {
        cylinder: true,
        position: [0.7, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [0.3, 0.6, 0.3],
        color: [0.05, 0.8, 0.05],
        cap_color: [0.8, 0.8, 0.8],
    }
}

#[test]
fn boxes_and_cylinders_draw_lit_where_placed() {
    let Some(data) = render(&[red_box(), green_can()], false) else {
        return;
    };
    // The box is left of centre, the can right of centre, both on the horizon line.
    let left = pixel(&data, SIZE / 2 - 13, SIZE / 2);
    let right = pixel(&data, SIZE / 2 + 15, SIZE / 2);
    // The target is sRGB: the 0.05 linear minor channels read back near 60, the major ones near 230.
    assert!(left[0] > 150 && left[1] < 100, "red box on the left: {left:?}");
    assert!(right[1] > 150 && right[0] < 100, "green can on the right: {right:?}");
    assert_eq!(pixel(&data, 2, 2), [0, 0, 0, 255], "background untouched");
    // Lighting varies across the box's faces.
    let shades: std::collections::BTreeSet<u8> =
        (SIZE / 2 - 22..SIZE / 2 - 4).map(|x| pixel(&data, x, SIZE / 2)[0]).filter(|&r| r > 100).collect();
    assert!(shades.len() > 1, "box faces are shaded differently: {shades:?}");
}

#[test]
fn props_write_depth_so_draws_behind_them_are_hidden() {
    let Some(data) = render(&[PropInstance { position: [0.0, 0.0, 0.0], ..red_box() }], true) else {
        return;
    };
    let middle = pixel(&data, SIZE / 2, SIZE / 2);
    assert!(middle[0] > 150 && middle[2] < 100, "the box hides the blue line behind it: {middle:?}");
    // The one-pixel line lands on a row next to the middle.
    let edge = (SIZE / 2 - 2..=SIZE / 2 + 2).flat_map(|y| (0..SIZE).map(move |x| (x, y))).any(|(x, y)| pixel(&data, x, y)[2] > 200);
    assert!(edge, "the line is visible beside the box");
}

#[test]
fn invalid_instances_are_rejected() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let mut renderer = PropRenderer::new(&device, COLOR_FORMAT);
    assert!(renderer.set(&queue, &[PropInstance { scale: [0.0, 1.0, 1.0], ..red_box() }]).is_err());
    assert!(renderer.set(&queue, &[PropInstance { color: [9.0, 0.0, 0.0], ..red_box() }]).is_err());
    assert!(renderer.set(&queue, &vec![red_box(); MAX_PROP_INSTANCES + 1]).is_err());
    renderer.set(&queue, &[red_box()]).unwrap();
    assert!(!renderer.is_empty());
}
