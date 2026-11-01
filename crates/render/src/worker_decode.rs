//! Checked staging for the opt-in worker experiment. Nothing enters the stream until complete.
use crate::sog::{self, Bundle, Meta, Texture};
use std::collections::BTreeMap;

pub struct StagedChunk {
    pub file: usize,
    meta: Meta,
    textures: BTreeMap<&'static str, Texture>,
    size: Option<(u32, u32)>,
}
impl StagedChunk {
    pub fn new(file: usize, meta: Meta) -> Self {
        Self {
            file,
            meta,
            textures: BTreeMap::new(),
            size: None,
        }
    }
    pub fn push(
        &mut self,
        index: usize,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), String> {
        if index != self.textures.len() || index >= sog::streams().len() {
            return Err("decoded streams out of order".into());
        }
        let name = sog::streams()[index].0;
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or("texture size overflow")?;
        if bytes != rgba.len() {
            return Err(format!("invalid {name} byte count"));
        }
        if index >= 5 && self.meta.sh_n_count == 0 {
            if width != 0 || height != 0 {
                return Err(format!("unexpected {name} for chunk without SH"));
            }
        } else if width == 0 || height == 0 {
            return Err(format!("empty {name}"));
        } else if index < 6 {
            if u64::from(width) * u64::from(height) < u64::from(self.meta.count) {
                return Err(format!("{name} shorter than splat count"));
            }
            match self.size {
                None => self.size = Some((width, height)),
                Some(size) if size != (width, height) => {
                    return Err(format!("{name} dimensions differ"))
                }
                _ => {}
            }
            if index == 5 && rgba.chunks_exact(4).any(|p| p[2] != 0) {
                return Err("invalid SH labels".into());
            }
        } else {
            let rows = self.meta.sh_n_count.div_ceil(64);
            // SOG stores 64 centroids per row, with 3, 8, or 15 coefficients
            // for one, two, or three spherical-harmonic bands respectively.
            if !matches!(width, 192 | 512 | 960)
                || height < rows
                || height > 1024
                || self.meta.sh_n_count > 65536
            {
                return Err("invalid SH centroid geometry".into());
            }
        }
        self.textures.insert(
            name,
            Texture {
                width,
                height,
                rgba,
            },
        );
        Ok(())
    }
    pub fn finish(self) -> Result<(usize, Bundle), String> {
        if self.textures.len() != sog::streams().len() {
            return Err("incomplete decoded chunk".into());
        }
        Ok((self.file, sog::complete(self.meta, self.textures)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta(sh: u32) -> Meta {
        Meta {
            count: 2,
            means_mins: [0.; 3],
            means_maxs: [1.; 3],
            scales_codebook: vec![],
            sh0_codebook: vec![],
            sh_n_codebook: vec![],
            sh_n_count: sh,
        }
    }
    #[test]
    fn checks_order_lengths_geometry_and_completeness() {
        let mut s = StagedChunk::new(1, meta(64));
        assert!(s.push(1, 2, 1, vec![0; 8]).is_err());
        assert!(s.push(0, 2, 1, vec![0; 7]).is_err());
        assert!(s.push(0, 1, 1, vec![0; 4]).is_err());
        s.push(0, 2, 1, vec![0; 8]).unwrap();
        assert!(s.push(1, 1, 2, vec![0; 8]).is_err());
        for i in 1..6 {
            s.push(i, 2, 1, vec![0; 8]).unwrap();
        }
        assert!(s.push(6, 1, 1, vec![0; 4]).is_err());
        s.push(6, 960, 1, vec![0; 3840]).unwrap();
        assert_eq!(s.finish().unwrap().0, 1);
        assert!(StagedChunk::new(2, meta(0)).finish().is_err());
    }
    #[test]
    fn accepts_each_supported_spherical_harmonic_band_layout() {
        for width in [192, 512, 960] {
            let mut staged = StagedChunk::new(0, meta(64));
            for index in 0..6 {
                staged.push(index, 2, 1, vec![0; 8]).unwrap();
            }
            staged
                .push(6, width, 1, vec![0; width as usize * 4])
                .unwrap();
            assert_eq!(
                staged.finish().unwrap().1.textures["sh_centroids"].width,
                width
            );
        }
    }
    #[test]
    fn rejects_unsupported_or_short_centroid_textures() {
        let mut staged = StagedChunk::new(0, meta(65));
        for index in 0..6 {
            staged.push(index, 2, 1, vec![0; 8]).unwrap();
        }
        assert!(staged.push(6, 256, 2, vec![0; 256 * 2 * 4]).is_err());
        assert!(staged.push(6, 192, 1, vec![0; 192 * 4]).is_err());
    }
    #[test]
    fn accepts_missing_sh_and_fills_standard_placeholders() {
        let mut s = StagedChunk::new(0, meta(0));
        for i in 0..5 {
            s.push(i, 2, 1, vec![0; 8]).unwrap();
        }
        for i in 5..7 {
            s.push(i, 0, 0, vec![]).unwrap();
        }
        let (_, b) = s.finish().unwrap();
        assert_eq!(b.textures["sh_labels"].rgba.len(), 8);
        assert_eq!(b.textures["sh_centroids"].width, 960);
    }
}
