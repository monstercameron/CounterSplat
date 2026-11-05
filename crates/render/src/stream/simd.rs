//! Vectorised arithmetic for the streaming upload hot path.
//!
//! The centre re-quantisation (`q = lo + 256·hi`, rescale against the shared range, round, clamp, look up the decoded
//! position) runs for every uploaded splat. The arithmetic half is written as branch-free fixed-size block loops so
//! LLVM emits NEON on aarch64 and `simd128` on wasm32 (the web build enables the feature); the 65,536-entry decode
//! lookup is a gather that no portable vector ISA has, so it stays scalar but runs over precomputed indices.
//!
//! Every result is bit-identical to the scalar expression in `decoded_shared_centre`: a block whose arithmetic could
//! differ (NaN, negative or negative-zero intermediates, which take the original `direct` path) is detected up front
//! and re-done with the scalar code, so correctness never depends on the vector path's edge-case behaviour.
use super::upload::decoded_shared_centre;
use crate::sog::Meta;

/// Splats per vector block. Large enough to amortise loop overhead and the per-block validity reduction.
pub(super) const BLOCK: usize = 64;

/// Rescaled 16-bit indices for one axis of one block, or `None` when any lane needs the exact scalar fallback.
#[inline(always)]
// `!(x >= 0.0)` is deliberate: it is true for NaN as well as for negatives.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn axis_indices(
    lo: &[u8],
    hi: &[u8],
    axis: usize,
    (scale, offset): (f32, f32),
) -> Option<[u32; BLOCK]> {
    let mut out = [0u32; BLOCK];
    let mut bad = 0u32;
    let lo = &lo[..BLOCK * 4];
    let hi = &hi[..BLOCK * 4];
    for k in 0..BLOCK {
        let q = f32::from(lo[k * 4 + axis]) + 256.0 * f32::from(hi[k * 4 + axis]);
        let x = q * scale + offset;
        // Valid lanes are exactly the non-negative numbers whose sign bit is clear (this rejects NaN and -0.0).
        bad |= u32::from(!(x >= 0.0)) | (x.to_bits() >> 31);
        // Round half away from zero for x >= 0, written with trunc/compare so it vectorises on every target
        // (wasm has no round-half-away instruction).
        let t = x.trunc();
        let r = t + if x - t >= 0.5 { 1.0 } else { 0.0 };
        out[k] = r.min(65535.0) as u32;
    }
    (bad == 0).then_some(out)
}

/// Fill `centres` from the packed 16-bit position streams. `lo`/`hi` hold RGBA texels (one splat each).
pub(super) fn centres(
    centres: &mut [[f32; 3]],
    lo: &[u8],
    hi: &[u8],
    factor: &[(f32, f32); 3],
    shared: &Meta,
    lookup: Option<&[Vec<f32>; 3]>,
) {
    let n = centres.len();
    let scalar = |centres: &mut [[f32; 3]], range: std::ops::Range<usize>| {
        for i in range {
            let s = i * 4;
            for c in 0..3 {
                let q = f32::from(lo[s + c]) + 256.0 * f32::from(hi[s + c]);
                let v = (q * factor[c].0 + factor[c].1)
                    .round()
                    .clamp(0.0, 65535.0);
                centres[i][c] = decoded_shared_centre(shared, c, v, lookup);
            }
        }
    };
    let tables: Option<[&[f32; 65536]; 3]> = lookup.and_then(|t| {
        Some([
            t[0].as_slice().try_into().ok()?,
            t[1].as_slice().try_into().ok()?,
            t[2].as_slice().try_into().ok()?,
        ])
    });
    let Some(tables) = tables else {
        scalar(centres, 0..n);
        return;
    };
    let mut i = 0;
    while i + BLOCK <= n {
        let (l, h) = (&lo[i * 4..], &hi[i * 4..]);
        let idx = [
            axis_indices(l, h, 0, factor[0]),
            axis_indices(l, h, 1, factor[1]),
            axis_indices(l, h, 2, factor[2]),
        ];
        if let [Some(x), Some(y), Some(z)] = &idx {
            for (k, out) in centres[i..i + BLOCK].iter_mut().enumerate() {
                *out = [
                    tables[0][(x[k] & 0xFFFF) as usize],
                    tables[1][(y[k] & 0xFFFF) as usize],
                    tables[2][(z[k] & 0xFFFF) as usize],
                ];
            }
        } else {
            scalar(centres, i..i + BLOCK);
        }
        i += BLOCK;
    }
    scalar(centres, i..n);
}
