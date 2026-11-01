//! Shared SOG codebook operations used by asset merging and streamed GPU uploads.

/// Returns 256 values spread by rank over the finite values in the source codebooks.
pub fn merge<'a>(books: impl Iterator<Item = &'a [f32]>) -> Vec<f32> {
    let mut all: Vec<f32> = books
        .flat_map(|book| book.iter().copied())
        .filter(|value| value.is_finite())
        .collect();
    all.sort_by(f32::total_cmp);
    if all.is_empty() {
        return vec![0.0; 256];
    }
    (0..256)
        .map(|index| all[(index * (all.len() - 1)) / 255])
        .collect()
}

/// Maps each source entry to its nearest target entry. The target must be finite, sorted and contain 1–256 values.
/// The remaining entries in the 256-byte map are zero when the source contains fewer than 256 values.
pub fn remap(from: &[f32], to: &[f32]) -> Result<[u8; 256], &'static str> {
    if from.len() > 256
        || to.is_empty()
        || to.len() > 256
        || !from.iter().all(|value| value.is_finite())
        || !to.iter().all(|value| value.is_finite())
        || to.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err("codebooks must be finite; target must be sorted and contain 1–256 entries");
    }
    let mut map = [0u8; 256];
    for (index, &value) in from.iter().enumerate() {
        let upper = to
            .partition_point(|&target| target < value)
            .min(to.len() - 1);
        let nearest = if upper > 0 && (value - to[upper - 1]).abs() <= (to[upper] - value).abs() {
            upper - 1
        } else {
            upper
        };
        map[index] = nearest as u8;
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_codebooks() {
        assert!(remap(&[1.0], &[]).is_err());
        assert!(remap(&[1.0], &[2.0, 1.0]).is_err());
        assert!(remap(&[f32::NAN], &[1.0]).is_err());
    }
}
