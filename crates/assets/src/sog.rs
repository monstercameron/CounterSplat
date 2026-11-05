//! Reads a SOG v2 bundle (a zip of `meta.json` plus WebP textures, as written by splat-transform), or the same files
//! unpacked in a directory, as a streamed level's chunks are.
//!
//! The decoded textures keep the engine's stream names (`means_l`, `sh_centroids`, ...) so they can be
//! bound by name against the table from `splat-wgsl`. The codebook LUT matches `_createCodebookTexture`
//! in the engine's `gsplat-sog-data.js`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// One decoded RGBA8 texture, row-major, top row first (the order WebP decoders produce).
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The parts of `meta.json` the renderer uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub count: u32,
    pub means_mins: [f32; 3],
    pub means_maxs: [f32; 3],
    pub scales_codebook: Vec<f32>,
    pub sh0_codebook: Vec<f32>,
    pub sh_n_codebook: Vec<f32>,
    /// Centroids in the SH palette (`shN.count`).
    pub sh_n_count: u32,
}

/// A loaded bundle: metadata plus every texture the SOG V2 SH stream layout needs.
#[derive(Debug)]
pub struct Bundle {
    pub meta: Meta,
    pub textures: BTreeMap<&'static str, Texture>,
}

/// Stream name → file inside the bundle. Matches `sog_streams(true, true)` in splat-wgsl.
const STREAM_FILES: [(&str, &str); 7] = [
    ("means_l", "means_l.webp"),
    ("means_u", "means_u.webp"),
    ("quats", "quats.webp"),
    ("scales", "scales.webp"),
    ("sh0", "sh0.webp"),
    ("sh_labels", "shN_labels.webp"),
    ("sh_centroids", "shN_centroids.webp"),
];

/// Loads and decodes a `.sog` bundle from disk.
pub fn load(path: &Path) -> Result<Bundle> {
    load_bytes(&std::fs::read(path)?)
}

/// Loads a `.sog` bundle from its bytes. The browser build uses this, since it has no file system.
pub fn load_bytes(bytes: &[u8]) -> Result<Bundle> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    load_parts(|name| read_entry(&mut archive, name))
}

/// Loads an unpacked bundle: a directory holding `meta.json` and the WebP files, such as a level chunk `2_0/`.
pub fn load_dir(dir: &Path) -> Result<Bundle> {
    load_parts(|name| {
        std::fs::read(dir.join(name))
            .map_err(|e| format!("{}: {e}", dir.join(name).display()).into())
    })
}

/// The files a bundle needs, `meta.json` first. The browser fetches these for each level chunk.
pub const BUNDLE_FILES: [&str; 8] = [
    "meta.json",
    "means_l.webp",
    "means_u.webp",
    "quats.webp",
    "scales.webp",
    "sh0.webp",
    "shN_labels.webp",
    "shN_centroids.webp",
];

/// Loads a bundle from its files by name (see `BUNDLE_FILES`).
pub fn load_parts(mut read: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<Bundle> {
    let meta = parse_meta_json(&read("meta.json")?)?;
    let mut textures = BTreeMap::new();
    for (stream, file) in STREAM_FILES {
        if is_sh_stream(stream) && meta.sh_n_count == 0 {
            continue;
        }
        textures.insert(stream, decode_stream(stream, &read(file)?)?);
    }
    Ok(complete(meta, textures))
}

/// Whether `stream` belongs to the spherical harmonics, which a bundle may leave out.
pub fn is_sh_stream(stream: &str) -> bool {
    stream == "sh_labels" || stream == "sh_centroids"
}

/// A bundle from its meta and decoded textures. A bundle without spherical harmonics (no `shN` in its meta) gets
/// zero labels and a one-row centroid texture whose entries decode to zero (its SH codebook is all zero), so the
/// shader, which always reads SH, adds nothing.
pub fn complete(meta: Meta, mut textures: BTreeMap<&'static str, Texture>) -> Bundle {
    let has = |t: &BTreeMap<&str, Texture>, name: &str| {
        t.get(name).is_some_and(|t| t.width > 0 && t.height > 0)
    };
    if !has(&textures, "sh_labels") || !has(&textures, "sh_centroids") {
        let (w, h) = textures
            .get("means_l")
            .map_or((1, 1), |t| (t.width, t.height));
        textures.insert(
            "sh_labels",
            Texture {
                width: w,
                height: h,
                rgba: vec![0; (w * h * 4) as usize],
            },
        );
        textures.insert(
            "sh_centroids",
            Texture {
                width: 64 * 15,
                height: 1,
                rgba: vec![0; 64 * 15 * 4],
            },
        );
    }
    Bundle { meta, textures }
}

/// The bundle files' streams, in `BUNDLE_FILES` order after `meta.json`: (stream name, file name).
pub fn streams() -> &'static [(&'static str, &'static str)] {
    &STREAM_FILES
}

/// Decodes one stream's WebP file. A caller with no threads (the browser) can decode a bundle a texture at a time
/// and assemble it with `Bundle { meta, textures }`.
pub fn decode_stream(stream: &str, bytes: &[u8]) -> Result<Texture> {
    if bytes.is_empty() && is_sh_stream(stream) {
        // A bundle without spherical harmonics; `complete` fills these in.
        return Ok(Texture {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        });
    }
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .map_err(|e| format!("decoding {stream}: {e}"))?
        // Most SOG streams already decode to RGBA. Consume that image instead of
        // cloning its whole pixel buffer; RGB WebPs still use the same conversion.
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    let mut rgba = decoded.into_raw();
    if stream == "sh_labels" {
        // Labels are 16-bit (red, green). The shader also reads blue as a third byte for merged levels, so it
        // must be zero here whatever the file holds.
        rgba.chunks_exact_mut(4).for_each(|t| t[2] = 0);
    }
    Ok(Texture {
        width,
        height,
        rgba,
    })
}

/// Splat centres in splat space, decoded from the means textures as the engine's `sogCenters` shader does:
/// `n = (l + 256·u) / 65535` per axis, `v = mix(mins, maxs, n)`, then the inverse of the log transform.
pub fn centres(bundle: &Bundle) -> Vec<[f32; 3]> {
    let meta = &bundle.meta;
    let (means_l, means_u) = (&bundle.textures["means_l"], &bundle.textures["means_u"]);
    let decode = |c: usize, q: u32| {
        let n = q as f32 / 65535.0;
        unlog(meta.means_mins[c] + (meta.means_maxs[c] - meta.means_mins[c]) * n)
    };
    let count = meta.count as usize;
    if count < 1 << 16 {
        return (0..count)
            .map(|i| {
                [0, 1, 2].map(|c| {
                    let q = u32::from(means_l.rgba[i * 4 + c]) + 256 * u32::from(means_u.rgba[i * 4 + c]);
                    decode(c, q)
                })
            })
            .collect();
    }
    // A position axis has only 65,536 distinct encoded values, so evaluate the `exp` once per value instead of
    // three times per splat (millions of calls on a real scan). The inputs to `decode` are identical, so every
    // centre is bit-for-bit what the direct expression gives.
    let tables: [Vec<f32>; 3] = [0, 1, 2].map(|c| (0..=u32::from(u16::MAX)).map(|q| decode(c, q)).collect());
    means_l.rgba[..count * 4]
        .chunks_exact(4)
        .zip(means_u.rgba[..count * 4].chunks_exact(4))
        .map(|(lo, hi)| {
            [0, 1, 2].map(|c| tables[c][(usize::from(lo[c]) | usize::from(hi[c]) << 8) & 0xFFFF])
        })
        .collect()
}

/// The mean of the splat centres, in splat space. Used to order a level's chunks back to front; the mean is less
/// sensitive than the bounds to the far-away floaters scans contain.
pub fn mean_centre(centres: &[[f32; 3]]) -> [f32; 3] {
    let n = centres.len().max(1) as f64;
    let mut sum = [0f64; 3];
    for c in centres {
        for i in 0..3 {
            sum[i] += f64::from(c[i]);
        }
    }
    sum.map(|s| (s / n) as f32)
}

/// Width of a merged bundle's per-splat textures.
const MERGED_WIDTH: u32 = 2048;
/// Largest texture side the merge produces, WebGPU's default limit.
const MAX_TEXTURE_SIDE: u32 = 8192;
/// SH centroids per row of the centroid texture, and texels per centroid (bands 1–3), as the shader reads them.
const CENTROIDS_PER_ROW: u32 = 64;
const SH_COEFFS: u32 = 15;

/// Merges several bundles (a level's chunks) into one, so a single sort orders every splat. Chunks overlap in space,
/// so drawing them one after another cannot be depth-correct.
///
/// - Positions are re-quantised to the union of the chunks' (log-space) ranges, 16 bits per axis as before.
/// - Each codebook (scales, sh0, shN) becomes 256 values spread over the union of the chunks' entries; every chunk
///   index is mapped to the nearest one. The chunks' codebooks are close, so the change is small.
/// - SH centroids are concatenated; labels gain a third byte (the renderer interprets the 24-bit labels).
///
/// Errors when the merged centroid texture would exceed `MAX_TEXTURE_SIDE` rows (too many chunks at one LOD).
pub fn merge(bundles: &[Bundle]) -> Result<Bundle> {
    let total: u64 = bundles.iter().map(|b| u64::from(b.meta.count)).sum();
    if bundles.is_empty() {
        return Err("nothing to merge".into());
    }
    let centroids: u64 = bundles.iter().map(|b| u64::from(b.meta.sh_n_count)).sum();
    let centroid_rows = centroids.div_ceil(u64::from(CENTROIDS_PER_ROW)) as u32;
    let splat_rows = total.div_ceil(u64::from(MERGED_WIDTH)) as u32;
    if centroid_rows > MAX_TEXTURE_SIDE || splat_rows > MAX_TEXTURE_SIDE || centroids > 1 << 24 {
        return Err(format!(
            "{total} splats with {centroids} SH centroids do not fit one merged bundle"
        )
        .into());
    }

    let mut means_mins = [f32::MAX; 3];
    let mut means_maxs = [f32::MIN; 3];
    for b in bundles {
        for i in 0..3 {
            means_mins[i] = means_mins[i].min(b.meta.means_mins[i]);
            means_maxs[i] = means_maxs[i].max(b.meta.means_maxs[i]);
        }
    }
    let scales_codebook =
        crate::codebook::merge(bundles.iter().map(|b| b.meta.scales_codebook.as_slice()));
    let sh0_codebook =
        crate::codebook::merge(bundles.iter().map(|b| b.meta.sh0_codebook.as_slice()));
    let sh_n_codebook =
        crate::codebook::merge(bundles.iter().map(|b| b.meta.sh_n_codebook.as_slice()));

    let splat_bytes = (MERGED_WIDTH * splat_rows * 4) as usize;
    let [mut means_l, mut means_u, mut quats, mut scales, mut sh0, mut labels] =
        [(); 6].map(|_| vec![0u8; splat_bytes]);
    let centroid_width = CENTROIDS_PER_ROW * SH_COEFFS;
    let mut centroid_texels = vec![0u8; (centroid_width * centroid_rows.max(1) * 4) as usize];

    let mut base = 0usize; // first merged splat index of this chunk
    let mut label_base = 0u32; // first merged centroid of this chunk
    for b in bundles {
        let m = &b.meta;
        let scale_map = crate::codebook::remap(&m.scales_codebook, &scales_codebook)?;
        let sh0_map = crate::codebook::remap(&m.sh0_codebook, &sh0_codebook)?;
        let sh_n_map = crate::codebook::remap(&m.sh_n_codebook, &sh_n_codebook)?;
        let t = |name: &str| b.textures[name].rgba.as_slice();
        let (src_l, src_u, src_q, src_s, src_c, src_n) = (
            t("means_l"),
            t("means_u"),
            t("quats"),
            t("scales"),
            t("sh0"),
            t("sh_labels"),
        );
        // Per axis: chunk value v = lo + span·q/65535 (log space), re-quantised as (v - merged lo) / merged span.
        let factor = [0, 1, 2].map(|c| {
            let span = (means_maxs[c] - means_mins[c]).max(f32::MIN_POSITIVE);
            (
                (m.means_maxs[c] - m.means_mins[c]) / span,
                (m.means_mins[c] - means_mins[c]) / span * 65535.0,
            )
        });
        for i in 0..m.count as usize {
            let (s, d) = (i * 4, (base + i) * 4);
            for c in 0..3 {
                let q = f32::from(src_l[s + c]) + 256.0 * f32::from(src_u[s + c]);
                let n = (q * factor[c].0 + factor[c].1).round().clamp(0.0, 65535.0) as u32;
                means_l[d + c] = (n & 255) as u8;
                means_u[d + c] = (n >> 8) as u8;
            }
            means_l[d + 3] = 255;
            means_u[d + 3] = 255;
            quats[d..d + 4].copy_from_slice(&src_q[s..s + 4]);
            scales[d..d + 4].copy_from_slice(&[
                scale_map[src_s[s] as usize],
                scale_map[src_s[s + 1] as usize],
                scale_map[src_s[s + 2] as usize],
                src_s[s + 3],
            ]);
            sh0[d..d + 4].copy_from_slice(&[
                sh0_map[src_c[s] as usize],
                sh0_map[src_c[s + 1] as usize],
                sh0_map[src_c[s + 2] as usize],
                src_c[s + 3],
            ]);
            let label = label_base + u32::from(src_n[s]) + 256 * u32::from(src_n[s + 1]);
            labels[d..d + 4].copy_from_slice(&[
                (label & 255) as u8,
                ((label >> 8) & 255) as u8,
                (label >> 16) as u8,
                255,
            ]);
        }
        // Centroids: rows of 64 × 15 texels, appended after the previous chunks' centroids.
        let centroids = &b.textures["sh_centroids"];
        for k in 0..m.sh_n_count {
            let (sx, sy) = ((k % CENTROIDS_PER_ROW) * SH_COEFFS, k / CENTROIDS_PER_ROW);
            if sy >= centroids.height {
                break;
            }
            let g = label_base + k;
            let (dx, dy) = ((g % CENTROIDS_PER_ROW) * SH_COEFFS, g / CENTROIDS_PER_ROW);
            for t in 0..SH_COEFFS {
                let s = ((sy * centroids.width + sx + t) * 4) as usize;
                let d = ((dy * centroid_width + dx + t) * 4) as usize;
                let texel = &centroids.rgba[s..s + 4];
                centroid_texels[d..d + 4].copy_from_slice(&[
                    sh_n_map[texel[0] as usize],
                    sh_n_map[texel[1] as usize],
                    sh_n_map[texel[2] as usize],
                    texel[3],
                ]);
            }
        }
        base += m.count as usize;
        label_base += m.sh_n_count;
    }
    let splat = |rgba: Vec<u8>| Texture {
        width: MERGED_WIDTH,
        height: splat_rows,
        rgba,
    };
    let textures = BTreeMap::from([
        ("means_l", splat(means_l)),
        ("means_u", splat(means_u)),
        ("quats", splat(quats)),
        ("scales", splat(scales)),
        ("sh0", splat(sh0)),
        ("sh_labels", splat(labels)),
        (
            "sh_centroids",
            Texture {
                width: centroid_width,
                height: centroid_rows.max(1),
                rgba: centroid_texels,
            },
        ),
    ]);
    let meta = Meta {
        count: total as u32,
        means_mins,
        means_maxs,
        scales_codebook,
        sh0_codebook,
        sh_n_codebook,
        sh_n_count: label_base,
    };
    Ok(Bundle { meta, textures })
}

/// Packs the scales (r), sh0 (g) and shN (b) codebooks into a 256x1 RGBA32F texture, as the engine does.
/// Returns 256 × 4 floats, row-major.
pub fn codebook_rgba32f(meta: &Meta) -> Vec<f32> {
    let mut data = vec![0.0f32; 256 * 4];
    for i in 0..256 {
        data[i * 4] = meta.scales_codebook[i];
        data[i * 4 + 1] = meta.sh0_codebook[i];
        data[i * 4 + 2] = meta.sh_n_codebook[i];
    }
    data
}

/// Maps a stored means value back to position space, as `calcAabb` / the engine's splat reader does.
pub fn unlog(v: f32) -> f32 {
    v.signum() * (v.abs().exp() - 1.0)
}

fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut entry = archive.by_name(name)?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Reads a bundle's `meta.json` on its own, for its counts, ranges and codebooks.
pub fn parse_meta_json(bytes: &[u8]) -> Result<Meta> {
    parse_meta(&serde_json::from_slice(bytes)?)
}

fn parse_meta(json: &serde_json::Value) -> Result<Meta> {
    if json["version"].as_u64() != Some(2) {
        return Err("only SOG version 2 bundles are supported".into());
    }
    let count = json["count"].as_u64().ok_or("meta.count missing")? as u32;
    let means = &json["means"];
    let means_mins = vec3(&means["mins"])?;
    let means_maxs = vec3(&means["maxs"])?;
    let scales_codebook = codebook(&json["scales"]["codebook"])?;
    let sh0_codebook = codebook(&json["sh0"]["codebook"])?;
    // Scans saved without spherical harmonics have no `shN` at all: an all-zero codebook and no centroids.
    let (sh_n_codebook, sh_n_count) = if json["shN"].is_null() {
        (vec![0.0; 256], 0)
    } else {
        (
            codebook(&json["shN"]["codebook"])?,
            json["shN"]["count"].as_u64().unwrap_or(65536) as u32,
        )
    };
    Ok(Meta {
        count,
        means_mins,
        means_maxs,
        scales_codebook,
        sh0_codebook,
        sh_n_codebook,
        sh_n_count,
    })
}

fn vec3(value: &serde_json::Value) -> Result<[f32; 3]> {
    let arr = value
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or("expected a 3-element array")?;
    Ok([0, 1, 2].map(|i| arr[i].as_f64().unwrap_or(0.0) as f32))
}

/// Reads a 256-entry codebook. The engine's `_patchCodebooks` fills a null first entry, so the same is done here.
fn codebook(value: &serde_json::Value) -> Result<Vec<f32>> {
    let raw = value
        .as_array()
        .filter(|a| a.len() == 256)
        .ok_or("codebook must have 256 entries")?;
    let mut out: Vec<f32> = raw
        .iter()
        .map(|v| v.as_f64().map_or(f32::NAN, |x| x as f32))
        .collect();
    if out[0].is_nan() {
        out[0] = out[1] + (out[1] - out[255]) / 255.0;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The previous conversion is retained here to check dimensions, every
    // pixel byte, label normalization and error messages against the new path.
    fn decode_stream_reference(stream: &str, bytes: &[u8]) -> Result<Texture> {
        if bytes.is_empty() && is_sh_stream(stream) {
            return Ok(Texture {
                width: 0,
                height: 0,
                rgba: Vec::new(),
            });
        }
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
            .map_err(|e| format!("decoding {stream}: {e}"))?
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        let mut rgba = decoded.into_raw();
        if stream == "sh_labels" {
            rgba.chunks_exact_mut(4).for_each(|t| t[2] = 0);
        }
        Ok(Texture {
            width,
            height,
            rgba,
        })
    }

    #[test]
    fn webp_conversion_matches_reference_for_rgb_rgba_and_errors() {
        use image::ExtendedColorType;

        let rgb = [
            0, 1, 255, 128, 17, 44, 90, 91, 92, 255, 128, 0, 11, 99, 180, 12, 15, 16,
        ];
        let rgba = [
            0, 1, 255, 0, 128, 17, 44, 1, 90, 91, 92, 127, 255, 128, 0, 128, 11, 99, 180, 254, 12,
            15, 16, 255,
        ];
        for (pixels, color) in [
            (rgb.as_slice(), ExtendedColorType::Rgb8),
            (rgba.as_slice(), ExtendedColorType::Rgba8),
        ] {
            let mut webp = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
                .encode(pixels, 3, 2, color)
                .unwrap();
            for stream in ["means_l", "sh_labels", "sh_centroids"] {
                let old = decode_stream_reference(stream, &webp).unwrap();
                let new = decode_stream(stream, &webp).unwrap();
                assert_eq!((new.width, new.height), (old.width, old.height));
                assert_eq!(new.rgba, old.rgba, "{stream}, {color:?}");
                if stream == "sh_labels" {
                    assert!(new.rgba.chunks_exact(4).all(|pixel| pixel[2] == 0));
                }
            }

            for stream in ["means_l", "sh_labels"] {
                for malformed in [&[][..], &webp[..4], &webp[..webp.len() / 2]] {
                    let old = decode_stream_reference(stream, malformed);
                    let new = decode_stream(stream, malformed);
                    match (old, new) {
                        (Ok(a), Ok(b)) => {
                            assert_eq!((a.width, a.height, a.rgba), (b.width, b.height, b.rgba))
                        }
                        (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
                        _ => panic!(
                            "different error behavior for {stream} and {} bytes",
                            malformed.len()
                        ),
                    }
                }
            }
        }
    }

    /// A bundle of `n` splats along x from `lo` to `hi` metres, each with its own centroid label.
    fn line(n: u32, lo: f32, hi: f32, codebook_shift: f32) -> Bundle {
        let log = |v: f32| v.signum() * (v.abs() + 1.0).ln();
        let (mins, maxs) = ([log(lo), -1.0, -1.0], [log(hi), 1.0, 1.0]);
        let mut textures = BTreeMap::new();
        let mut means_l = vec![0u8; (n * 4) as usize];
        let mut means_u = vec![0u8; (n * 4) as usize];
        let mut labels = vec![0u8; (n * 4) as usize];
        for i in 0..n as usize {
            let x = lo + (hi - lo) * i as f32 / (n - 1) as f32;
            let q = (((log(x) - mins[0]) / (maxs[0] - mins[0])) * 65535.0).round() as u32;
            means_l[i * 4] = (q & 255) as u8;
            means_u[i * 4] = (q >> 8) as u8;
            for c in 1..3 {
                means_l[i * 4 + c] = 255; // 32767.5 / 65535 ≈ mid-range, so y = z ≈ 0
                means_u[i * 4 + c] = 127;
            }
            labels[i * 4] = i as u8;
            labels[i * 4 + 2] = 0;
        }
        let plain = || vec![7u8; (n * 4) as usize];
        textures.insert(
            "means_l",
            Texture {
                width: n,
                height: 1,
                rgba: means_l,
            },
        );
        textures.insert(
            "means_u",
            Texture {
                width: n,
                height: 1,
                rgba: means_u,
            },
        );
        for name in ["quats", "scales", "sh0"] {
            textures.insert(
                name,
                Texture {
                    width: n,
                    height: 1,
                    rgba: plain(),
                },
            );
        }
        textures.insert(
            "sh_labels",
            Texture {
                width: n,
                height: 1,
                rgba: labels,
            },
        );
        textures.insert(
            "sh_centroids",
            Texture {
                width: 64 * 15,
                height: 1,
                rgba: vec![3u8; 64 * 15 * 4],
            },
        );
        let book: Vec<f32> = (0..256).map(|i| i as f32 * 0.01 + codebook_shift).collect();
        let meta = Meta {
            count: n,
            means_mins: mins,
            means_maxs: maxs,
            scales_codebook: book.clone(),
            sh0_codebook: book.clone(),
            sh_n_codebook: book,
            sh_n_count: 64,
        };
        Bundle { meta, textures }
    }

    #[test]
    fn merged_centres_match_the_chunks_and_labels_are_offset() {
        let (a, b) = (line(50, -40.0, -2.0, 0.0), line(40, 1.0, 60.0, 0.5));
        let merged = merge(&[a, b]).expect("merge");
        let (a, b) = (line(50, -40.0, -2.0, 0.0), line(40, 1.0, 60.0, 0.5));
        assert_eq!(merged.meta.count, 90);
        assert_eq!(merged.meta.sh_n_count, 128);
        let expected: Vec<[f32; 3]> = centres(&a).into_iter().chain(centres(&b)).collect();
        for (got, want) in centres(&merged).iter().zip(&expected) {
            // 16 bits over the merged log range: about 1e-4 relative, plus the original quantisation.
            assert!(
                (got[0] - want[0]).abs() <= 0.01 + want[0].abs() * 5e-4,
                "{got:?} vs {want:?}"
            );
        }
        let labels = &merged.textures["sh_labels"].rgba;
        let label = |i: usize| {
            u32::from(labels[i * 4])
                + 256 * u32::from(labels[i * 4 + 1])
                + 65536 * u32::from(labels[i * 4 + 2])
        };
        assert_eq!(label(3), 3);
        assert_eq!(
            label(50 + 3),
            64 + 3,
            "the second chunk's labels follow the first chunk's centroids"
        );
    }

    #[test]
    fn table_decoded_centres_are_bit_identical_to_the_direct_expression() {
        // Large enough to take the 65,536-entry table path; the small bundle takes the direct one.
        for n in [100u32, 70_000] {
            let mut bundle = line(n, -40.0, 60.0, 0.0);
            let mut state = 12345u32;
            for name in ["means_l", "means_u"] {
                for byte in &mut bundle.textures.get_mut(name).unwrap().rgba {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    *byte = (state >> 24) as u8;
                }
            }
            let meta = &bundle.meta;
            let (l, u) = (&bundle.textures["means_l"], &bundle.textures["means_u"]);
            let got = centres(&bundle);
            assert_eq!(got.len(), n as usize);
            for (i, centre) in got.iter().enumerate() {
                for c in 0..3 {
                    let lo = f32::from(l.rgba[i * 4 + c]);
                    let hi = f32::from(u.rgba[i * 4 + c]);
                    let n = (lo + 256.0 * hi) / 65535.0;
                    let want =
                        unlog(meta.means_mins[c] + (meta.means_maxs[c] - meta.means_mins[c]) * n);
                    assert_eq!(centre[c].to_bits(), want.to_bits(), "splat {i} axis {c}");
                }
            }
        }
    }

    #[test]
    fn merged_codebooks_keep_values_close() {
        let books = [vec![0.0f32, 1.0, 2.0], vec![0.5f32, 1.5, 2.5]];
        let merged = crate::codebook::merge(books.iter().map(|b| b.as_slice()));
        assert_eq!(merged.len(), 256);
        let map = crate::codebook::remap(&books[1], &merged).unwrap();
        for (i, v) in books[1].iter().enumerate() {
            assert!(
                (merged[map[i] as usize] - v).abs() < 1e-6,
                "{v} maps to {}",
                merged[map[i] as usize]
            );
        }
    }
}
