//! Exercise the real GPU effect pipeline, including surface depth occlusion.
use countersplat_render::{
    camera::IDENTITY,
    impact::{ImpactRenderer, MAX_IMPACT_SPRITES},
    COLOR_FORMAT, DEPTH_FORMAT,
};

fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &ImpactRenderer,
    depth_value: f32,
) -> Vec<u8> {
    let extent = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let output = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.8,
                        g: 0.8,
                        b: 0.8,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(depth_value),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.draw(queue, &mut pass, &IDENTITY, &IDENTITY);
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
        extent,
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap().to_vec();
    readback.unmap();
    data
}

#[test]
fn decals_and_smoke_draw_and_respect_depth() {
    let Some((device, queue)) = countersplat_render::try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut effects = ImpactRenderer::new(&device, COLOR_FORMAT);
    assert!(effects.is_empty());
    assert!(effects.set(&queue, &[0.0; 11]).is_err());
    assert!(effects
        .set(&queue, &vec![0.0; 12 * (MAX_IMPACT_SPRITES + 1)])
        .is_err());
    assert!(effects
        .set(&queue, &[0., 0., 0.5, 0.2, 0., 0., 0., 0., 1., 1., 0., 0.])
        .is_err());
    effects
        .set(
            &queue,
            &[
                -0.5, 0., 0.5, 0.32, 0., 0., 1., 0., 1., 1., 0., 0., 0.5, 0., 0.5, 0.32, 0., 0.,
                1., 1., 0.7, 2., 0., 0.,
            ],
        )
        .unwrap();
    let visible = render(&device, &queue, &effects, 1.0);
    let hidden = render(&device, &queue, &effects, 0.2);
    let pixel = |data: &[u8], x: usize, y: usize| data[(y * 64 + x) * 4];
    assert!(
        pixel(&visible, 16, 32) + 30 < pixel(&hidden, 16, 32),
        "decal must darken the surface"
    );
    assert!(
        pixel(&visible, 48, 32) + 5 < pixel(&hidden, 48, 32),
        "smoke must blend over the surface"
    );
    assert_eq!(pixel(&visible, 0, 0), pixel(&hidden, 0, 0));
    effects.set(&queue, &[]).unwrap();
    assert!(effects.is_empty());
}
