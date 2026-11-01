//! CPU-side re-encoding of streamed chunks into the shared GPU pool format.
use super::{Remap, PALETTE_CENTROIDS};
use crate::lod::Range;
use crate::sog::{self, Bundle, Meta};

/// A shared range never changes after stream creation. Build its exact 16-bit decoded centres once, only when a
/// range is actually uploaded. Invalid ranges keep the original direct calculation.
pub(super) enum CentreLookup {
    Uninitialized,
    Direct,
    Table([Vec<f32>; 3]),
}

impl CentreLookup {
    pub(super) fn for_upload(&mut self, shared: &Meta) -> Option<&[Vec<f32>; 3]> {
        if matches!(self, Self::Uninitialized) {
            *self = make_centre_lookup(shared).map_or(Self::Direct, Self::Table);
        }
        match self {
            Self::Table(table) => Some(table),
            Self::Direct => None,
            Self::Uninitialized => unreachable!(),
        }
    }
}

pub(super) fn direct_shared_centre(shared: &Meta, axis: usize, value: f32) -> f32 {
    sog::unlog(
        shared.means_mins[axis]
            + (shared.means_maxs[axis] - shared.means_mins[axis]) * value / 65535.0,
    )
}

pub(super) fn make_centre_lookup(shared: &Meta) -> Option<[Vec<f32>; 3]> {
    if (0..3).any(|c| {
        !shared.means_mins[c].is_finite()
            || !shared.means_maxs[c].is_finite()
            || !(shared.means_maxs[c] - shared.means_mins[c]).is_finite()
    }) {
        return None;
    }
    Some([0, 1, 2].map(|c| {
        (0..=u16::MAX)
            .map(|value| direct_shared_centre(shared, c, f32::from(value)))
            .collect()
    }))
}

pub(super) fn decoded_shared_centre(
    shared: &Meta,
    axis: usize,
    value: f32,
    lookup: Option<&[Vec<f32>; 3]>,
) -> f32 {
    // Round/clamp above produces integral values in 0..=65535, except for NaN or negative zero. The bit check also
    // keeps those edge cases on the original expression, including its zero sign and NaN behavior.
    if let Some(table) = lookup.filter(|_| value.to_bits() <= 65535.0f32.to_bits()) {
        table[axis][value as usize]
    } else {
        direct_shared_centre(shared, axis, value)
    }
}

#[derive(Default)]
pub(super) struct EncodedUpload {
    pub(super) centres: Vec<[f32; 3]>,
    pub(super) scales: Vec<u8>,
    pub(super) sh0: Vec<u8>,
    pub(super) labels: Vec<u8>,
}

/// CPU preparation for a pool upload, separated from the GPU write so it can be checked and profiled independently.
pub(super) fn encode_upload_into(
    encoded: &mut EncodedUpload,
    b: &Bundle,
    remap: &Remap,
    shared: &Meta,
    range: Range,
    palette: Option<usize>,
    lookup: Option<&[Vec<f32>; 3]>,
) {
    let n = range.count as usize;
    let (first, last) = (range.offset as usize * 4, (range.offset as usize + n) * 4);
    let src = |name: &str| &b.textures[name].rgba[first..last.min(b.textures[name].rgba.len())];
    let (src_l, src_u, src_s, src_c, src_n) = (
        src("means_l"),
        src("means_u"),
        src("scales"),
        src("sh0"),
        src("sh_labels"),
    );
    // Per axis: chunk value v = lo + span·q/65535 (log space), re-quantised against the shared range.
    let factor = [0, 1, 2].map(|c| {
        let span = (shared.means_maxs[c] - shared.means_mins[c]).max(f32::MIN_POSITIVE);
        (
            (remap.meta.means_maxs[c] - remap.meta.means_mins[c]) / span,
            (remap.meta.means_mins[c] - shared.means_mins[c]) / span * 65535.0,
        )
    });
    // Reserve exactly before resizing so repeated modest uploads can reuse storage
    // without geometric growth retaining far more memory than the upload needs.
    fn resize<T: Clone>(vec: &mut Vec<T>, len: usize, value: T) {
        if vec.capacity() < len {
            vec.reserve_exact(len - vec.len());
        }
        vec.resize(len, value);
    }
    resize(&mut encoded.scales, n * 4, 0);
    resize(&mut encoded.sh0, n * 4, 0);
    resize(&mut encoded.labels, n * 4, 0);
    resize(&mut encoded.centres, n, [0.0; 3]);
    let label_base = palette.map(|p| 64 + p as u32 * PALETTE_CENTROIDS);
    let available = n.min(src_l.len() / 4);
    for i in 0..available {
        let s = i * 4;
        for c in 0..3 {
            let q = f32::from(src_l[s + c]) + 256.0 * f32::from(src_u[s + c]);
            let v = (q * factor[c].0 + factor[c].1).round().clamp(0.0, 65535.0);
            encoded.centres[i][c] = decoded_shared_centre(shared, c, v, lookup);
        }
        encoded.scales[s..s + 4].copy_from_slice(&[
            remap.scales[src_s[s] as usize],
            remap.scales[src_s[s + 1] as usize],
            remap.scales[src_s[s + 2] as usize],
            src_s[s + 3],
        ]);
        encoded.sh0[s..s + 4].copy_from_slice(&[
            remap.sh0[src_c[s] as usize],
            remap.sh0[src_c[s + 1] as usize],
            remap.sh0[src_c[s + 2] as usize],
            src_c[s + 3],
        ]);
        let label = label_base.map_or(0, |base| {
            base + u32::from(src_n[s]) + 256 * u32::from(src_n[s + 1])
        });
        encoded.labels[s..s + 4].copy_from_slice(&[
            (label & 255) as u8,
            ((label >> 8) & 255) as u8,
            (label >> 16) as u8,
            255,
        ]);
    }
    // A short means stream leaves zeroed slots, including after a larger
    // previous upload reused these buffers.
    encoded.centres[available..].fill([0.0; 3]);
    encoded.scales[available * 4..].fill(0);
    encoded.sh0[available * 4..].fill(0);
    encoded.labels[available * 4..].fill(0);
}

/// Fresh allocation is useful as an independent call for tests and probes.
#[cfg(test)]
pub(super) fn encode_upload(
    b: &Bundle,
    remap: &Remap,
    shared: &Meta,
    range: Range,
    palette: Option<usize>,
    lookup: Option<&[Vec<f32>; 3]>,
) -> EncodedUpload {
    let mut encoded = EncodedUpload::default();
    encode_upload_into(&mut encoded, b, remap, shared, range, palette, lookup);
    encoded
}
