//! CPU-only Radiance HDR panorama decoding for environment assets.
use image::{codecs::hdr::HdrDecoder, DynamicImage, ImageDecoder};
use std::io::Cursor;

/// A linear-light, row-major equirectangular panorama, with the top row first.
#[derive(Debug, Clone)]
pub struct Equirect {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[f32; 3]>,
}

impl Equirect {
    /// Validate dimensions and linear RGB values before conversion or GPU upload.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.width == 0
            || self.height == 0
            || self.width != self.height.saturating_mul(2)
            || self.width.checked_mul(self.height) != Some(self.rgb.len())
            || self.rgb.len() > MAX_PIXELS
        {
            return Err("HDR panorama must be a nonempty 2:1 image within the pixel limit");
        }
        if !self
            .rgb
            .iter()
            .flatten()
            .all(|v| v.is_finite() && *v >= 0.0)
        {
            return Err("HDR panorama contains non-finite or negative radiance");
        }
        Ok(())
    }
}
/// Bounds decoding before allocating the floating-point image.
pub const MAX_PIXELS: usize = 16_777_216;
pub const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

/// Decode a Radiance `.hdr` RGBE file from bytes, preserving HDR linear-light radiance.
pub fn decode_hdr(bytes: &[u8]) -> Result<Equirect, String> {
    if bytes.is_empty() || bytes.len() > MAX_ENCODED_BYTES {
        return Err("HDR file is empty or exceeds 64 MiB".into());
    }
    let decoder = HdrDecoder::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if height == 0 || u64::from(width) != u64::from(height) * 2 || pixels > MAX_PIXELS as u64 {
        return Err("HDR panorama must be a nonempty 2:1 image within the pixel limit".into());
    }
    let image = DynamicImage::from_decoder(decoder)
        .map_err(|error| error.to_string())?
        .into_rgb32f();
    let rgb = image
        .into_raw()
        .chunks_exact(3)
        .map(|v| [v[0], v[1], v[2]])
        .collect();
    let result = Equirect {
        width: width as usize,
        height: height as usize,
        rgb,
    };
    result.validate().map_err(str::to_owned)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::hdr::HdrEncoder, Rgb};
    #[test]
    fn decodes_hdr_radiance_and_rejects_bad_assets() {
        let mut bytes = Vec::new();
        HdrEncoder::new(&mut bytes)
            .encode(&vec![Rgb([0.25, 1.0, 4.0]); 32], 8, 4)
            .unwrap();
        let pano = decode_hdr(&bytes).unwrap();
        assert_eq!((pano.width, pano.height), (8, 4));
        assert!(
            pano.rgb[0][2] > 3.9,
            "HDR values must not be clipped to one"
        );
        assert!(decode_hdr(b"not an hdr file").is_err());
        let mut wrong = Vec::new();
        HdrEncoder::new(&mut wrong)
            .encode(&vec![Rgb([1.0, 1.0, 1.0]); 16], 4, 4)
            .unwrap();
        assert!(decode_hdr(&wrong).is_err());
        let invalid = Equirect {
            width: 4,
            height: 2,
            rgb: vec![[f32::NAN, 0.0, 0.0]; 8],
        };
        assert!(invalid.validate().is_err());
    }
}
