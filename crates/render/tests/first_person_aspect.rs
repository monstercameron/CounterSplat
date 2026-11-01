//! Test first-person footage anchoring with the real GPU compositor.
use countersplat_render::first_person::{FirstPersonLayer, Settings};

#[test]
fn widescreen_footage_reaches_bottom_of_a_four_by_three_scene() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut layer = FirstPersonLayer::new(&device, countersplat_render::COLOR_FORMAT);
    let texture = layer.frame_texture(&device, 16, 9);
    let pixels: Vec<u8> = [255, 0, 0, 255].repeat(16 * 9);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(64),
            rows_per_image: Some(9),
        },
        wgpu::Extent3d {
            width: 16,
            height: 9,
            depth_or_array_layers: 1,
        },
    );
    layer.mark_ready();
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("first person aspect output"),
        size: wgpu::Extent3d {
            width: 64,
            height: 48,
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
        size: 256 * 48,
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
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    layer.encode(
        &queue,
        &mut encoder,
        &view,
        (64, 48),
        Settings {
            visible: true,
            ..Default::default()
        },
    );
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
                rows_per_image: Some(48),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 48,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap();
    let pixel = &data[47 * 256 + 32 * 4..47 * 256 + 32 * 4 + 4];
    assert!(
        pixel[0] > 200,
        "footage must extend to the bottom edge: {pixel:?}"
    );
}

#[test]
fn clip_handoffs_blend_keyed_frames_on_the_gpu() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        return;
    };
    let mut layer = FirstPersonLayer::new(&device, countersplat_render::COLOR_FORMAT);
    assert!(!layer.capture_previous(&device, &queue));
    let upload = |layer: &mut FirstPersonLayer, color: [u8; 4]| {
        let pixels = color.repeat(16 * 9);
        let texture = layer.frame_texture(&device, 16, 9);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(64),
                rows_per_image: Some(9),
            },
            wgpu::Extent3d {
                width: 16,
                height: 9,
                depth_or_array_layers: 1,
            },
        );
        layer.mark_ready();
    };
    upload(&mut layer, [255, 0, 0, 255]);
    assert!(layer.capture_previous(&device, &queue));
    upload(&mut layer, [0, 0, 255, 255]);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 48,
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
        size: 256 * 48,
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
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    layer.encode(
        &queue,
        &mut encoder,
        &view,
        (64, 48),
        Settings {
            visible: true,
            blend: 0.5,
            motion_x: 0.01,
            motion_roll: 0.01,
            ..Default::default()
        },
    );
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
                rows_per_image: Some(48),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 48,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap();
    let offset = 35 * 256 + 32 * 4;
    let pixel = &data[offset..offset + 4];
    assert!(
        pixel[0] > 60 && pixel[2] > 60 && pixel[1] < 10,
        "both keyed frames must blend: {pixel:?}"
    );
}
