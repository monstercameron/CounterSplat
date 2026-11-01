//! Verify that the UI composites onto an existing GPU scene target.
use countersplat_ui::{Color, Rect, ResolvedNode, Style, TextAlign, UiElement, UiFrame, Widget};
use countersplat_ui_wgpu::{UiRenderer, UiTarget};

#[test]
fn draws_shapes_and_fonts_over_the_scene_without_clearing_it() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("no GPU adapter available; skipping UI compositing test");
        return;
    };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&Default::default()))
    else {
        eprintln!("no GPU device available; skipping UI compositing test");
        return;
    };
    let size = wgpu::Extent3d {
        width: 128,
        height: 64,
        depth_or_array_layers: 1,
    };
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI test scene"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI test readback"),
        size: 128 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint existing scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.5,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let shape = Rect::new(8.0, 8.0, 48.0, 40.0);
    let text = Rect::new(65.0, 8.0, 55.0, 32.0);
    let graph = Rect::new(65.0, 42.0, 55.0, 18.0);
    let node = |id: &str, rect, element: UiElement| ResolvedNode {
        id: id.into(),
        rect,
        clip: rect,
        scroll_parent: None,
        drag_bounds: rect,
        drag_correction: [0.0, 0.0],
        z: 0,
        enabled: true,
        element,
    };
    let frame = UiFrame {
        nodes: vec![
            node(
                "image",
                Rect::new(0.0, 52.0, 22.0, 12.0),
                UiElement {
                    rect: Rect::new(0.0, 52.0, 22.0, 12.0),
                    widget: Widget::Image {
                        source: "level".into(),
                    },
                    style: Style {
                        border: Color::TRANSPARENT,
                        rounding: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            node(
                "shape",
                shape,
                UiElement {
                    rect: shape,
                    widget: Widget::Rectangle,
                    style: Style {
                        fill: Color([250, 20, 20, 255]),
                        border: Color::TRANSPARENT,
                        rounding: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            node(
                "text",
                text,
                UiElement {
                    rect: text,
                    widget: Widget::Text {
                        text: "UI".into(),
                        align: TextAlign::Left,
                    },
                    style: Style {
                        text: Color::WHITE,
                        font_size: 24.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            node(
                "graph",
                graph,
                UiElement {
                    rect: graph,
                    widget: Widget::LineGraph {
                        points: vec![[0.0, 0.5], [1.0, 0.5]],
                        x_min: 0.0,
                        x_max: 1.0,
                        y_min: 0.0,
                        y_max: 1.0,
                    },
                    style: Style {
                        rounding: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
        ],
    };
    let mut renderer = UiRenderer::new(&device, format);
    assert!(renderer.set_image_rgba("level", [2, 2], &[0; 3]).is_err());
    renderer
        .set_image_rgba(
            "level",
            [2, 2],
            &[
                0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
            ],
        )
        .unwrap();
    let output = renderer
        .draw(
            UiTarget {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                color: &view,
                size: [128, 64],
                pixels_per_point: 1.0,
                time_seconds: 0.0,
            },
            &frame,
        )
        .unwrap();
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(512),
                rows_per_image: Some(64),
            },
        },
        size,
    );
    queue.submit(
        output
            .command_buffers
            .into_iter()
            .chain(std::iter::once(encoder.finish())),
    );
    renderer.after_submit();
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let pixels = slice.get_mapped_range().unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * 128 + x) * 4..(y * 128 + x + 1) * 4];
    assert!(
        pixel(2, 2)[2] > 100 && pixel(2, 2)[0] < 5,
        "scene background was not preserved"
    );
    assert!(
        pixel(20, 20)[0] > 200 && pixel(20, 20)[2] < 30,
        "UI rectangle did not composite: shape={:?}, background={:?}",
        pixel(20, 20),
        pixel(2, 2)
    );
    assert!(
        pixel(10, 58)[1] > 200 && pixel(10, 58)[0] < 30,
        "UI image did not composite: {:?}",
        pixel(10, 58)
    );
    let text_pixels = (8..40)
        .flat_map(|y| (65..120).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(x, y)[0] > 40)
        .count();
    assert!(
        pixel(90, 51)[0] > 100 && pixel(90, 51)[1] > 70,
        "line graph did not render: {:?}",
        pixel(90, 51)
    );
    assert!(
        text_pixels > 4,
        "font glyphs were not drawn: {text_pixels} pixels"
    );
}
