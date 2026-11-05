//! Level map rules: validation and coalescing of a scene's `hud.map_request`, the splat-centre fallback raster for
//! levels without a voxel collider, the voxel work bound, LOD choice and the map's colours. The browser host runs the
//! raster on a worker; nothing here touches the GPU or a worker API.

use serde_json::Value;

/// A validated map request (`hud.map_request`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    pub position: [f64; 3],
    pub feet: f64,
    pub units: f64,
    pub radius: f64,
    pub height: f64,
    pub thickness: f64,
    pub size: u32,
}

fn within(value: Option<&Value>, low: f64, high: f64) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite() && (low..=high).contains(v))
}

const WORLD: f64 = 10_000_000.0;

impl Request {
    pub fn parse(value: &Value) -> Option<Self> {
        let position = value.get("position")?.as_array().filter(|p| p.len() == 3)?;
        let mut xyz = [0.0; 3];
        for (out, v) in xyz.iter_mut().zip(position) {
            *out = within(Some(v), -WORLD, WORLD)?;
        }
        let size = value.get("size")?.as_u64().filter(|s| (16..=256).contains(s))? as u32;
        Some(Self {
            position: xyz,
            feet: within(value.get("feet"), -WORLD, WORLD)?,
            units: within(value.get("units"), 0.5, 2.5)?,
            radius: within(value.get("radius"), 0.1, 100.0)?,
            height: within(value.get("height"), 0.0, 10.0)?,
            thickness: within(value.get("thickness"), 0.01, 2.0)?,
            size,
        })
    }

    /// The point a floor probe starts from: the eye, but no lower than just above the calibrated feet.
    pub fn probe(&self) -> [f32; 3] {
        let [x, y, z] = self.position;
        [x as f32, y.max(self.feet + 0.1 * self.units) as f32, z as f32]
    }

    /// The floor probe's horizontal sample radius and maximum drop.
    pub fn probe_reach(&self) -> (f32, f32) {
        ((0.15 * self.units).min(1.0) as f32, (6.0 * self.units).min(16.0) as f32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Loading,
    Ready,
    Unavailable,
}

/// Page-side request coalescing: one slice in flight, only the newest waiting request kept, and nothing sent until
/// the worker has loaded the level's geometry.
#[derive(Debug, Clone)]
pub struct Coalescer {
    state: State,
    pending: Option<Request>,
    in_flight: Option<u64>,
    sequence: u64,
}

impl Default for Coalescer {
    fn default() -> Self {
        Self { state: State::Loading, pending: None, in_flight: None, sequence: 0 }
    }
}

impl Coalescer {
    pub fn unavailable(&self) -> bool {
        self.state == State::Unavailable
    }

    /// Queues a request (replacing any waiting one); false when the map is unavailable.
    pub fn request(&mut self, request: Request) -> bool {
        if self.unavailable() {
            return false;
        }
        self.pending = Some(request);
        true
    }

    /// The next request to send, with its id.
    pub fn next(&mut self) -> Option<(u64, Request)> {
        if self.state != State::Ready || self.in_flight.is_some() {
            return None;
        }
        let request = self.pending.take()?;
        self.sequence += 1;
        self.in_flight = Some(self.sequence);
        Some((self.sequence, request))
    }

    pub fn ready(&mut self) {
        if self.state == State::Loading {
            self.state = State::Ready;
        }
    }

    pub fn fail(&mut self) {
        self.state = State::Unavailable;
        self.pending = None;
        self.in_flight = None;
    }

    /// A reply (slice or slice error) for `id`; false (ignore it) when it is not the request in flight.
    pub fn reply(&mut self, id: Option<u64>) -> bool {
        if id.is_none() || id != self.in_flight {
            return false;
        }
        self.in_flight = None;
        true
    }

    pub fn stop(&mut self) {
        self.pending = None;
        self.in_flight = None;
    }
}

/// Decodes SOG mean positions (16-bit interpolation and inverse log, as the renderer does), negating X and Y when
/// the level is rotated 180° about Z.
pub fn decode_splat_centres(meta: &Value, low: &[u8], high: &[u8], flip_xy: bool) -> Result<Vec<f32>, String> {
    let invalid = || "Invalid SOG means for map".to_string();
    let count = meta.get("count").and_then(Value::as_u64).filter(|c| (1..=3_000_000).contains(c)).ok_or_else(invalid)? as usize;
    let axes = |key| -> Option<[f64; 3]> {
        let values = meta.get("means")?.get(key)?.as_array().filter(|v| v.len() == 3)?;
        let mut out = [0.0; 3];
        for (o, v) in out.iter_mut().zip(values) {
            *o = v.as_f64().filter(|v| v.is_finite())?;
        }
        Some(out)
    };
    let (min, max) = (axes("mins").ok_or_else(invalid)?, axes("maxs").ok_or_else(invalid)?);
    if low.len() < count * 4 || high.len() < count * 4 {
        return Err(invalid());
    }
    let mut points = vec![0.0f32; count * 3];
    for i in 0..count {
        for axis in 0..3 {
            let q = (f64::from(low[i * 4 + axis]) + 256.0 * f64::from(high[i * 4 + axis])) / 65535.0;
            let v = min[axis] + (max[axis] - min[axis]) * q;
            let value = v.signum() * v.abs().exp_m1();
            points[i * 3 + axis] = (if flip_xy && axis < 2 { -value } else { value }) as f32;
        }
    }
    Ok(points)
}

/// North-up splat-centre occupancy (world X right, Z down) of the slab `height ± thickness/2`. Each centre marks its
/// pixel at full strength and its neighbours at 96, so sparse scans stay legible without inventing walls. This
/// shows observed splat centres, not collision or walkable space.
pub fn rasterize_splat_slice(chunks: &[Vec<f32>], position: [f64; 3], height: f64, radius: f64, thickness: f64, size: u32) -> Vec<u8> {
    let size_i = size as i64;
    let mut mask = vec![0u8; (size * size) as usize];
    let factor = f64::from(size) / (radius * 2.0);
    let half = thickness * 0.5;
    for points in chunks {
        for p in points.chunks_exact(3) {
            let (px, py, pz) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            if !py.is_finite() || (py - height).abs() > half {
                continue;
            }
            let x = ((px - position[0] + radius) * factor).floor();
            let y = ((pz - position[2] + radius) * factor).floor();
            if !(0.0..f64::from(size)).contains(&x) || !(0.0..f64::from(size)).contains(&y) {
                continue;
            }
            let (x, y) = (x as i64, y as i64);
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let (qx, qy) = (x + ox, y + oy);
                    if (0..size_i).contains(&qx) && (0..size_i).contains(&qy) {
                        let cell = &mut mask[(qy * size_i + qx) as usize];
                        *cell = (*cell).max(if ox == 0 && oy == 0 { 255 } else { 96 });
                    }
                }
            }
        }
    }
    mask
}

/// One chunk's decoded splat centres grouped into height buckets (a counting sort), with the heights in their own
/// array. A map slab keeps only the centres within `thickness / 2` of its height, a few percent of a level:
/// `rasterize_splat_columns` reads just the buckets the slab overlaps and applies the original test to their centres,
/// where `rasterize_splat_slice` streams every centre's 12 bytes through the cache on each request. Built once per
/// decoded chunk, in linear time.
#[derive(Debug, Clone, Default)]
pub struct SplatColumn {
    heights: Vec<f32>,
    xz: Vec<[f32; 2]>,
    /// Bucket b holds the centres `starts[b]..starts[b + 1]`; heights from `low` in steps of `step`.
    starts: Vec<u32>,
    low: f64,
    step: f64,
}

impl SplatColumn {
    /// From `decode_splat_centres` output (x, y, z per centre). Centres whose height is not finite never pass the
    /// slab test, so they are left out.
    pub fn new(points: &[f32]) -> Self {
        let finite = || points.chunks_exact(3).filter(|p| p[1].is_finite());
        let (low, high) = finite().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(f64::from(p[1])), hi.max(f64::from(p[1])))
        });
        let count = finite().count();
        if count == 0 {
            return Self::default();
        }
        let buckets = (count / 64).clamp(1, 4096);
        let step = ((high - low) / buckets as f64).max(f64::MIN_POSITIVE);
        let bucket = |y: f32| (((f64::from(y) - low) / step) as usize).min(buckets - 1);
        let mut starts = vec![0u32; buckets + 1];
        for p in finite() {
            starts[bucket(p[1]) + 1] += 1;
        }
        for b in 0..buckets {
            starts[b + 1] += starts[b];
        }
        let mut fill = starts.clone();
        let (mut heights, mut xz) = (vec![0f32; count], vec![[0f32; 2]; count]);
        for p in finite() {
            let at = &mut fill[bucket(p[1])];
            heights[*at as usize] = p[1];
            xz[*at as usize] = [p[0], p[2]];
            *at += 1;
        }
        Self { heights, xz, starts, low, step }
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// The centres a slab `height ± half` may keep: the buckets it overlaps, widened by one on each side so rounding
    /// in the bucket arithmetic can never leave a kept centre out. The caller applies the exact test to each.
    fn candidates(&self, height: f64, half: f64) -> std::ops::Range<usize> {
        let buckets = self.starts.len().saturating_sub(1);
        if buckets == 0 || !height.is_finite() || !half.is_finite() {
            return 0..self.heights.len();
        }
        let index = |y: f64| ((y - self.low) / self.step).floor();
        let first = (index(height - half) - 1.0).clamp(0.0, buckets as f64) as usize;
        let last = (index(height + half) + 2.0).clamp(0.0, buckets as f64) as usize;
        self.starts[first] as usize..self.starts[last.max(first)] as usize
    }
}
/// Marks `(x, y)` at full strength and its neighbours at 96, keeping the strongest mark per pixel.
#[inline]
fn stamp(mask: &mut [u8], size: i64, x: i64, y: i64) {
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (qx, qy) = (x + ox, y + oy);
            if (0..size).contains(&qx) && (0..size).contains(&qy) {
                let cell = &mut mask[(qy * size + qx) as usize];
                *cell = (*cell).max(if ox == 0 && oy == 0 { 255 } else { 96 });
            }
        }
    }
}

/// `rasterize_splat_slice` over height-bucketed columns: the same image (each pixel keeps its strongest mark, so the
/// order the centres are visited in does not matter, and each candidate gets the same slab test), reading only the
/// buckets near the slab.
pub fn rasterize_splat_columns(
    columns: &[SplatColumn],
    position: [f64; 3],
    height: f64,
    radius: f64,
    thickness: f64,
    size: u32,
) -> Vec<u8> {
    let size_i = size as i64;
    let mut mask = vec![0u8; (size * size) as usize];
    let factor = f64::from(size) / (radius * 2.0);
    let half = thickness * 0.5;
    for column in columns {
        let range = column.candidates(height, half);
        for (&py, &[px, pz]) in column.heights[range.clone()].iter().zip(&column.xz[range]) {
            if (f64::from(py) - height).abs() > half {
                continue;
            }
            let x = ((f64::from(px) - position[0] + radius) * factor).floor();
            let y = ((f64::from(pz) - position[2] + radius) * factor).floor();
            if !(0.0..f64::from(size)).contains(&x) || !(0.0..f64::from(size)).contains(&y) {
                continue;
            }
            stamp(&mut mask, size_i, x as i64, y as i64);
        }
    }
    mask
}

/// Caps a voxel slice's radius so a large map on a fine grid stays within about 14 M cell visits (boundary cells
/// counted conservatively).
pub fn bounded_voxel_radius(radius: f64, thickness: f64, resolution: f64) -> f64 {
    if !radius.is_finite() || !thickness.is_finite() || !resolution.is_finite() || resolution <= 0.0 {
        return radius;
    }
    let y_cells = (thickness / resolution).ceil() + 2.0;
    let safe = (((14_000_000.0 / y_cells).sqrt() - 2.0) * resolution * 0.5).max(0.1);
    radius.min(safe)
}

/// The chunk descriptors a splat map reads: a single SOG (no LOD manifest) has one; a streamed level uses the
/// finest LOD with at most 2.5 M splats (else the coarsest), up to 32 chunks.
pub fn select_splat_map_files(manifest: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(manifest) = manifest else {
        return Ok(vec!["meta.json".into()]);
    };
    let invalid = || "invalid map LOD manifest".to_string();
    let coarsest = manifest.get("lodLevels").and_then(Value::as_i64).map(|n| n - 1).filter(|c| (0..=31).contains(c)).ok_or_else(invalid)?;
    let names = manifest.get("filenames").and_then(Value::as_array).ok_or_else(invalid)?;
    let candidate = manifest.get("counts").and_then(Value::as_array).and_then(|counts| {
        counts.iter().position(|n| n.as_u64().is_some_and(|n| n > 0 && n <= 2_500_000))
    });
    let lod = candidate.map(|c| c as i64).filter(|&c| c <= coarsest).unwrap_or(coarsest);
    let prefix = format!("{lod}_");
    let files: Vec<String> = names
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| name.starts_with(&prefix) && chunk_descriptor(name))
        .map(String::from)
        .collect();
    if files.is_empty() || files.len() > 32 {
        return Err("no bounded map LOD".into());
    }
    Ok(files)
}

/// `<digits>_<digits>/meta.json`
fn chunk_descriptor(name: &str) -> bool {
    let Some(dir) = name.strip_suffix("/meta.json") else {
        return false;
    };
    let mut parts = dir.split('_');
    let digits = |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// RGBA map image from an occupancy mask: a round map (radius 0.97) with occupied cells light and translucent by
/// strength, free cells dark, outside the circle clear.
pub fn colorize(mask: &[u8], size: u32) -> Vec<u8> {
    let size_u = size as usize;
    let mut pixels = vec![0u8; size_u * size_u * 4];
    for y in 0..size_u {
        for x in 0..size_u {
            let i = y * size_u + x;
            let dx = (x as f64 + 0.5) / f64::from(size) * 2.0 - 1.0;
            let dy = (y as f64 + 0.5) / f64::from(size) * 2.0 - 1.0;
            if dx * dx + dy * dy > 0.97 * 0.97 {
                continue;
            }
            let rgba = match mask[i] {
                0 => [10, 20, 25, 178],
                m => [190, 211, 208, (135.0 + 90.0 * f64::from(m) / 255.0).round() as u8],
            };
            pixels[i * 4..i * 4 + 4].copy_from_slice(&rgba);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Random centres spread over 40 x 30 x 40, with non-finite heights and centres exactly on slab boundaries mixed in.
    fn cloud(n: usize, seed: u32) -> Vec<f32> {
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / 16_777_216.0
        };
        (0..n)
            .flat_map(|i| {
                let y = match i % 97 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => 1.0 + 0.2, // exactly on the upper boundary of the height-1.0, thickness-0.4 slab
                    3 => 1.0 - 0.2,
                    _ => next() * 30.0 - 5.0,
                };
                [next() * 40.0 - 20.0, y, next() * 40.0 - 20.0]
            })
            .collect()
    }

    #[test]
    fn height_bucketed_columns_draw_the_same_map_as_the_full_scan() {
        let chunks: Vec<Vec<f32>> = (0..3).map(|k| cloud(20_000 + k * 777, 7 + k as u32)).collect();
        let columns: Vec<SplatColumn> = chunks.iter().map(|c| SplatColumn::new(c)).collect();
        let finite: usize = chunks.iter().map(|c| c.chunks_exact(3).filter(|p| p[1].is_finite()).count()).sum();
        assert_eq!(columns.iter().map(SplatColumn::len).sum::<usize>(), finite);
        let mut lit = 0;
        for (height, thickness) in [(1.0, 0.4), (-4.9, 0.4), (12.37, 2.0), (24.99, 0.01), (100.0, 0.4), (3.0, 60.0)] {
            for (position, radius, size) in [([0.0, 0.0, 0.0], 12.0, 128), ([7.5, 0.0, -3.25], 4.0, 64), ([-19.0, 0.0, 19.0], 30.0, 256)] {
                let expected = rasterize_splat_slice(&chunks, position, height, radius, thickness, size);
                let actual = rasterize_splat_columns(&columns, position, height, radius, thickness, size);
                assert_eq!(actual, expected, "height {height}, thickness {thickness}, position {position:?}");
                lit += expected.iter().filter(|&&v| v > 0).count();
            }
        }
        assert!(lit > 10_000, "the maps are not empty");
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_splat_map_slab() {
        use std::{hint::black_box, time::Instant};
        let chunks = vec![cloud(2_500_000, 3)];
        let columns: Vec<SplatColumn> = chunks.iter().map(|c| SplatColumn::new(c)).collect();
        let time = |f: &dyn Fn() -> Vec<u8>| {
            let mut best = f64::MAX;
            for _ in 0..9 {
                let start = Instant::now();
                black_box(f());
                best = best.min(start.elapsed().as_secs_f64() * 1e3);
            }
            best
        };
        let scan = time(&|| rasterize_splat_slice(black_box(&chunks), [0.0; 3], 1.0, 12.0, 0.4, 128));
        let slab = time(&|| rasterize_splat_columns(black_box(&columns), [0.0; 3], 1.0, 12.0, 0.4, 128));
        let start = Instant::now();
        black_box(SplatColumn::new(&chunks[0]));
        println!(
            "2.5 M centres, 0.4 slab: full scan {scan:.2} ms, height-bucketed {slab:.3} ms; bucketing once {:.1} ms",
            start.elapsed().as_secs_f64() * 1e3
        );
    }

    fn request(x: f64) -> Request {
        Request::parse(&json!({"position":[x,2,0],"feet":0.3,"units":1,"radius":12,"height":0.9144,"thickness":0.4,"size":32})).unwrap()
    }

    #[test]
    fn requests_validate_and_coalesce() {
        assert!(Request::parse(&json!({"position":[0,2,0],"feet":0.3,"units":1,"radius":"inf","height":0.9,"thickness":0.4,"size":32})).is_none());
        assert!(Request::parse(&json!({"position":[0,2],"feet":0.3,"units":1,"radius":12,"height":0.9,"thickness":0.4,"size":32})).is_none());
        assert!(Request::parse(&json!({"position":[0,2,0],"feet":0.3,"units":1,"radius":12,"height":0.9,"thickness":0.4,"size":32.5})).is_none());
        let mut map = Coalescer::default();
        map.request(request(0.0));
        map.request(request(1.0));
        assert!(map.next().is_none(), "requests wait until the worker is ready");
        map.ready();
        let (id, sent) = map.next().unwrap();
        assert_eq!(sent.position[0], 1.0, "loading requests coalesce");
        map.request(request(2.0));
        map.request(request(3.0));
        assert!(map.next().is_none(), "only one request is in flight");
        assert!(!map.reply(Some(999)), "stale replies are ignored");
        assert!(map.reply(Some(id)));
        assert_eq!(map.next().unwrap().1.position[0], 3.0, "only the newest waiting request is sent");
        map.fail();
        assert!(!map.request(request(4.0)) && map.next().is_none());
    }

    #[test]
    fn splat_centres_and_raster() {
        let meta = json!({"count":2,"means":{"mins":[0,0,0],"maxs":[1,1,1]}});
        let low = [0, 0, 0, 0, 255, 255, 255, 0];
        let high = [0, 0, 0, 0, 255, 255, 255, 0];
        let points = decode_splat_centres(&meta, &low, &high, true).unwrap();
        let e = (1.0f64.exp_m1()) as f32;
        assert_eq!(points[..3], [-0.0, -0.0, 0.0]);
        assert!((points[3] + e).abs() < 1e-6 && (points[4] + e).abs() < 1e-6 && (points[5] - e).abs() < 1e-6);
        assert!(decode_splat_centres(&json!({"count":3,"means":{"mins":[0,0,0],"maxs":[1,1,1]}}), &low, &high, false).is_err());

        let mask = rasterize_splat_slice(&[vec![0.0, 1.0, 0.0, 5.0, 9.0, 5.0]], [0.0, 2.0, 0.0], 1.0, 4.0, 0.4, 16);
        assert_eq!(mask[8 * 16 + 8], 255);
        assert_eq!(mask[7 * 16 + 7], 96);
        assert_eq!(mask.iter().filter(|&&m| m > 0).count(), 9, "the out-of-slab point is not drawn");
        let pixels = colorize(&mask, 16);
        assert_eq!(pixels[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4], [190, 211, 208, 225]);
        assert_eq!(pixels[(8 * 16 + 1) * 4 + 3], 178);
        assert_eq!(pixels[3], 0, "corners are outside the round map");
    }

    #[test]
    fn voxel_bound_and_lod_choice() {
        assert_eq!(bounded_voxel_radius(12.0, 0.4, 0.1), 12.0);
        assert!(bounded_voxel_radius(100.0, 0.4, 0.01) < 100.0);
        assert_eq!(bounded_voxel_radius(5.0, 0.4, 0.0), 5.0);
        assert_eq!(select_splat_map_files(None).unwrap(), ["meta.json"]);
        let manifest = json!({"lodLevels":3,"counts":[9_000_000,2_000_000,100],"filenames":["0_0/meta.json","1_0/meta.json","1_1/meta.json","2_0/meta.json","1_x/meta.json"]});
        assert_eq!(select_splat_map_files(Some(&manifest)).unwrap(), ["1_0/meta.json", "1_1/meta.json"]);
        let coarsest = json!({"lodLevels":2,"filenames":["0_0/meta.json","1_0/meta.json"]});
        assert_eq!(select_splat_map_files(Some(&coarsest)).unwrap(), ["1_0/meta.json"]);
        assert!(select_splat_map_files(Some(&json!({"lodLevels":0,"filenames":[]}))).is_err());
        assert!(select_splat_map_files(Some(&json!({"lodLevels":4,"filenames":["../../outside"]}))).is_err());
        let traversal = json!({"lodLevels":3,"counts":[5_000_000,2_000_000,1_000_000],"filenames":["1_0/meta.json","1_0/../../other"]});
        assert_eq!(select_splat_map_files(Some(&traversal)).unwrap(), ["1_0/meta.json"]);
        let bounded = bounded_voxel_radius(75.0, 2.0, 0.05);
        assert!(bounded < 75.0 && ((2.0 * bounded / 0.05).ceil() + 2.0).powi(2) * ((2.0f64 / 0.05).ceil() + 2.0) < 16_000_000.0);
    }

    #[test]
    fn inverse_log_and_north_up_coordinates() {
        let (l2, l3, l4) = (2f64.ln(), 3f64.ln(), 4f64.ln());
        let meta = json!({"count":1,"means":{"mins":[l2,-l3,l4],"maxs":[l2,-l3,l4]}});
        let points = decode_splat_centres(&meta, &[0; 4], &[0; 4], true).unwrap();
        assert!((points[0] + 1.0).abs() < 1e-5 && (points[1] - 2.0).abs() < 1e-5 && (points[2] - 3.0).abs() < 1e-5);
        let mask = rasterize_splat_slice(&[points, vec![-1.0, 5.0, 3.0]], [-1.0, 0.0, 3.0], 2.0, 4.0, 0.4, 16);
        assert_eq!(mask[8 * 16 + 8], 255);
        assert_eq!(mask.iter().filter(|&&m| m == 255).count(), 1, "only the requested height is projected");
        assert_eq!(mask.iter().filter(|&&m| m > 0).count(), 9);
        assert!(decode_splat_centres(&meta, &[0; 3], &[0; 4], true).is_err());
        assert!(decode_splat_centres(&json!({"count":3_000_001,"means":{"mins":[0,0,0],"maxs":[0,0,0]}}), &[0; 4], &[0; 4], true).is_err());
    }
}
