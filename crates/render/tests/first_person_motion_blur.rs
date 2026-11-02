//! Test the keyed weapon-only shutter filter with real GPU readback.
use countersplat_render::first_person::{FirstPersonLayer, Settings};

#[test]
fn directional_blur_softens_only_the_keyed_weapon_and_preserves_clip_blending() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut layer = FirstPersonLayer::new(&device, countersplat_render::COLOR_FORMAT);
    let upload = |layer: &mut FirstPersonLayer, color: [u8; 4]| {
        let mut pixels = vec![0u8; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let i = (y * 64 + x) * 4;
                pixels[i..i + 4].copy_from_slice(
                    if (24..40).contains(&x) && (16..48).contains(&y) {
                        &color
                    } else {
                        &[0, 255, 0, 255]
                    },
                );
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: layer.frame_texture(&device, 64, 64),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        layer.mark_ready();
    };
    upload(&mut layer, [255, 0, 0, 255]);
    let render = |layer: &FirstPersonLayer, settings: Settings| -> Vec<u8> {
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("weapon blur test"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: countersplat_render::COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 1.0,
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
        layer.encode(&queue, &mut encoder, &view, (64, 64), settings);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output,
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
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range().unwrap().to_vec();
        bytes
    };
    let pixel = |img: &[u8], x: usize, y: usize| -> [u8; 4] {
        img[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4]
            .try_into()
            .unwrap()
    };
    let plain = Settings {
        visible: true,
        vertical_offset: 0.0,
        ..Default::default()
    };
    let base = render(&layer, plain);
    let subpixel = render(
        &layer,
        Settings {
            blur_x: 0.005,
            ..plain
        },
    );
    assert_eq!(
        subpixel, base,
        "subpixel turns should use the inexpensive sharp path"
    );
    let short = render(
        &layer,
        Settings {
            blur_x: 0.02,
            ..plain
        },
    );
    assert!(
        pixel(&short, 23, 32)[0] > 0,
        "a short trail uses the reduced tap path"
    );
    let horizontal = render(
        &layer,
        Settings {
            blur_x: 0.04,
            ..plain
        },
    );
    let vertical = render(
        &layer,
        Settings {
            blur_y: 0.04,
            ..plain
        },
    );
    let bg = pixel(&base, 22, 32);
    let edge = pixel(&horizontal, 22, 32);
    assert!(
        bg[0] == 0 && edge[0] > 0 && edge[2] > 0 && edge[1] < 5,
        "horizontal red blur over blue scene without green: {bg:?} {edge:?}"
    );
    assert_eq!(
        pixel(&vertical, 22, 32),
        bg,
        "vertical blur must not spread sideways"
    );
    assert!(
        pixel(&vertical, 32, 14)[0] > 0 && pixel(&horizontal, 32, 14)[0] == 0,
        "vertical blur must spread vertically"
    );
    assert_eq!(
        pixel(&horizontal, 5, 32),
        pixel(&base, 5, 32),
        "world away from the weapon stays untouched"
    );
    assert_eq!(
        pixel(&horizontal, 32, 32),
        pixel(&base, 32, 32),
        "a flat weapon interior remains stable"
    );
    assert!(layer.capture_previous(&device, &queue));
    upload(&mut layer, [255, 255, 255, 255]);
    let blend = render(
        &layer,
        Settings {
            blur_x: 0.04,
            blend: 0.5,
            ..plain
        },
    );
    let inside = pixel(&blend, 32, 32);
    assert!(
        inside[0] > 200 && inside[1] > 80 && inside[2] > 80,
        "clip blend must remain intact: {inside:?}"
    );
    let blended_edge = pixel(&blend, 22, 32);
    assert!(
        blended_edge[0] > 0 && blended_edge[1] > 0 && blended_edge[1] < blended_edge[2],
        "blur must include blended clip colors: {blended_edge:?}"
    );
}
