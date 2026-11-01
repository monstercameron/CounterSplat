//! Render-pass setup shared by scene hosts.

/// A colour pass over `color`, tested against `depth`. `clear_color` clears the colour to black (else it loads);
/// `clear_depth` clears the depth to the far plane (else it loads what an earlier pass wrote).
pub fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    clear_color: bool,
    clear_depth: bool,
    label: &str,
) -> wgpu::RenderPass<'e> {
    color_pass_timed(encoder, color, depth, clear_color, clear_depth, label, None)
}

fn color_pass_timed<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    clear_color: bool,
    clear_depth: bool,
    label: &str,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear_color {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: if clear_depth {
                    wgpu::LoadOp::Clear(1.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A depth-only pass that clears `depth` to the far plane, for `GsplatScene::draw_depth`.
pub fn depth_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    depth: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'e> {
    depth_pass_timed(encoder, depth, label, None)
}
fn depth_pass_timed<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    depth: &wgpu::TextureView,
    label: &str,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}
