//! GPU texture uploads, quad geometry, and byte encoding helpers.
use super::INSTANCE_SIZE;

/// Four vertices per splat slot; six indices form two triangles over the quad corners in [-1, 1]².
/// Each vertex is `(x, y, slot)`, where `slot` becomes `vertex_position.z` (the offset within an instance).
pub(super) fn quad_vertices() -> Vec<f32> {
    const CORNERS: [(f32, f32); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let mut out = Vec::with_capacity((INSTANCE_SIZE as usize) * 4 * 3);
    for slot in 0..INSTANCE_SIZE {
        for (x, y) in CORNERS {
            out.extend_from_slice(&[x, y, slot as f32]);
        }
    }
    out
}

/// Writes `count` texels from slot `start` of a texture laid out `width` slots per row: the partial first row, the
/// full rows in one copy, then the partial last row.
pub(super) fn write_slots(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    start: u32,
    count: u32,
    texels: &[u8],
) {
    let write = |x: u32, y: u32, w: u32, h: u32, from: usize| {
        let bytes = (w * h * 4) as usize;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &texels[from..from + bytes],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    };
    let (mut slot, end, mut from) = (start, start + count, 0usize);
    if slot % width != 0 {
        let w = (width - slot % width).min(end - slot);
        write(slot % width, slot / width, w, 1, from);
        slot += w;
        from += w as usize * 4;
    }
    let full = (end - slot) / width;
    if full > 0 {
        write(0, slot / width, width, full, from);
        slot += full * width;
        from += (full * width) as usize * 4;
    }
    if slot < end {
        write(0, slot / width, end - slot, 1, from);
    }
}

/// Creates a texture and, with `data`, fills it.
pub(super) fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    data: Option<&[u8]>,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let Some(data) = data else { return texture };
    let bytes_per_texel = format.block_copy_size(None).expect("uncompressed format");
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_texel),
            rows_per_image: Some(height),
        },
        size,
    );
    texture
}

pub(super) fn f32s(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The GPU expects little-endian IEEE-754 values. On little-endian hosts, borrowing the existing centres preserves
/// every bit without two temporary copies. Big-endian targets use the explicit conversion from the original path.
pub(super) fn centres_le_bytes(centres: &[[f32; 3]]) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(target_endian = "little")]
    {
        std::borrow::Cow::Borrowed(bytemuck::cast_slice(centres))
    }
    #[cfg(target_endian = "big")]
    {
        std::borrow::Cow::Owned(centres_to_le_bytes(centres))
    }
}

#[cfg(any(test, target_endian = "big"))]
pub(super) fn centres_to_le_bytes(centres: &[[f32; 3]]) -> Vec<u8> {
    centres
        .iter()
        .flat_map(|centre| centre.iter().flat_map(|value| value.to_le_bytes()))
        .collect()
}

pub(super) fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
