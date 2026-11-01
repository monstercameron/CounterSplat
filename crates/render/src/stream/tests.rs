use super::*;

/// A row of `n` leaves along x, 10 m apart, each with two LODs in files 0 (fine) and 1 (coarse).
fn tree(n: usize) -> LodMeta {
    let leaves = (0..n)
        .map(|i| crate::lod::Leaf {
            min: [i as f32 * 10.0, 0.0, 0.0],
            max: [i as f32 * 10.0 + 5.0, 1.0, 1.0],
            lods: vec![
                Some(Range {
                    file: 0,
                    offset: i as u32 * 100,
                    count: 100,
                }),
                Some(Range {
                    file: 1,
                    offset: i as u32 * 10,
                    count: 10,
                }),
            ],
        })
        .collect();
    LodMeta {
        counts: vec![100 * n as u64, 10 * n as u64],
        filenames: vec!["0_0/meta.json".into(), "1_0/meta.json".into()],
        leaves,
    }
}

fn meta_json() -> Vec<u8> {
    let book: Vec<String> = (0..256).map(|i| format!("{}", i as f32 * 0.01)).collect();
    let book = book.join(",");
    format!(r#"{{"version":2,"count":1,"means":{{"mins":[-1,-1,-1],"maxs":[1,1,1]}},"scales":{{"codebook":[{book}]}},"sh0":{{"codebook":[{book}]}},"shN":{{"count":64,"codebook":[{book}]}}}}"#).into_bytes()
}

fn stream(capacity: u32) -> LevelStream {
    let settings = StreamSettings {
        capacity,
        lod_distance_m: 12.0,
        lod_error_px: 0.0,
        behind_bias: 0,
        ..StreamSettings::default()
    };
    LevelStream::new(tree(6), &[Some(meta_json()), Some(meta_json())], settings).unwrap()
}

// Retained pre-optimization selection and level calculations. Comparing the full private state exercises
// resident ranges, keep-delay and frustum hysteresis without needing a graphics device.
impl LevelStream {
    fn select_reference(
        &mut self,
        eye: [f32; 3],
        forward: [f32; 3],
        clip: Option<&[f32; 16]>,
        now_s: f32,
    ) {
        self.frame += 1;
        self.now = now_s;
        let keep_s = self.settings.keep_s;
        let count =
            |leaf: &crate::lod::Leaf, lod: usize| u64::from(leaf.lods[lod].map_or(0, |r| r.count));
        let planes = clip.map(|c| {
            (
                frustum_planes(c, ENTER_MARGIN),
                frustum_planes(c, LEAVE_MARGIN),
            )
        });
        let mut budget = i64::from(self.settings.capacity) * SELECT_SHARE / 100;
        let mut targets = vec![None; self.leaves.len()];
        let levels: Vec<f32> = (0..self.leaves.len())
            .map(|i| {
                let leaf = &self.meta.leaves[i];
                self.level_at_reference(i, effective_distance(eye, leaf.min, leaf.max))
            })
            .collect();
        for (i, (state, leaf)) in self.leaves.iter_mut().zip(&self.meta.leaves).enumerate() {
            let distance = effective_distance(eye, leaf.min, leaf.max);
            state.distance = distance;
            if distance > self.settings.max_distance_m {
                continue;
            }
            let Some(coarse) = coarsest_available(&leaf.lods) else {
                continue;
            };
            // In view: inside the frustum (entering by the narrow margin, leaving by the wide one), or without one, not
            // entirely behind the camera. Leaves near the camera always count.
            state.visible = distance <= self.settings.any_view_m
                || match &planes {
                    Some((enter, leave)) => box_in_frustum(
                        if state.visible { leave } else { enter },
                        leaf.min,
                        leaf.max,
                    ),
                    None => {
                        let centre = [0, 1, 2].map(|i| (leaf.min[i] + leaf.max[i]) * 0.5);
                        let radius = (0..3)
                            .map(|i| (leaf.max[i] - leaf.min[i]).powi(2))
                            .sum::<f32>()
                            .sqrt()
                            * 0.5;
                        (0..3)
                            .map(|i| (centre[i] - eye[i]) * forward[i])
                            .sum::<f32>()
                            >= -radius
                    }
                };
            // Continuous level (`level_at`), plus the bias out of view. The bias goes in before the hysteresis, so both
            // compare the same level.
            let bias = if state.visible {
                0.0
            } else {
                self.settings.behind_bias as f32
            };
            let x = levels[i] + bias;
            let mut lod = x.floor() as usize;
            // Hysteresis: keep the resident LOD while within a fifth of a level of its band.
            if let Some(have) = state.have {
                if x > have.lod as f32 - 0.2 && x < have.lod as f32 + 1.2 {
                    lod = have.lod;
                }
            }
            // Going coarser waits `keep_s`: the leaf may be back in view, or the budget back, by then.
            match state.have {
                Some(have) if lod > have.lod => {
                    let since = *state.coarsen_since.get_or_insert(now_s);
                    if now_s - since < keep_s {
                        lod = have.lod;
                    }
                }
                _ => state.coarsen_since = None,
            }
            targets[i] = Some((lod.min(coarse), coarse));
            budget -= count(leaf, coarse) as i64;
        }
        let mut order: Vec<usize> = (0..self.leaves.len())
            .filter(|&i| targets[i].is_some())
            .collect();
        // Leaves in view first, then nearest, a leaf already at its target LOD counting as nearer.
        let priority = |i: usize| {
            let state = &self.leaves[i];
            let settled = state
                .have
                .is_some_and(|h| targets[i].is_some_and(|(t, _)| h.lod <= t));
            state.distance * if settled { STICKY } else { 1.0 }
        };
        order.sort_by(|&a, &b| {
            self.leaves[b]
                .visible
                .cmp(&self.leaves[a].visible)
                .then(priority(a).total_cmp(&priority(b)))
        });
        for &i in &order {
            let (target, coarse) = targets[i].expect("filtered");
            let leaf = &self.meta.leaves[i];
            let base = count(leaf, coarse) as i64;
            // The finest LOD from the target down that exists and fits what is left.
            let chosen = (target..=coarse)
                .filter(|&l| leaf.lods[l].is_some())
                .find(|&l| count(leaf, l) as i64 - base <= budget)
                .unwrap_or(coarse);
            budget -= count(leaf, chosen) as i64 - base;
            self.leaves[i].want = Some(chosen);
        }
        for (i, state) in self.leaves.iter_mut().enumerate() {
            if targets[i].is_none() {
                state.want = None;
            }
        }
        self.sharp_m = self
            .leaves
            .iter()
            .filter(|s| s.visible && s.want.is_some_and(|w| w >= 2))
            .map(|s| s.distance)
            .fold(f32::MAX, f32::min);
        // How much of the configured detail the pool allowed: the share of leaves that got their distance's LOD.
        let refined = order
            .iter()
            .filter(|&&i| self.leaves[i].want == targets[i].map(|t| t.0))
            .count();
        self.distance_scale = if order.is_empty() {
            1.0
        } else {
            refined as f32 / order.len() as f32
        };
        // When the pool cannot hold every leaf's target, leaves at the edge of the budget trade detail back and forth
        // as the camera moves. Shrinking the LOD distance until the targets fit makes them stable again (the
        // hysteresis and keep delay then hold); spare room lets it grow back. Slowly, so it does not oscillate.
        // Between "a few leaves short" and "15 % of the pool spare" it holds still, so it settles instead of hunting.
        // Crossfades cut short for room count as a shortage too: the selection must leave them space.
        let short =
            (order.len() - refined) * 50 > order.len() || std::mem::take(&mut self.squeezed);
        let spare = budget > i64::from(self.settings.capacity) * 15 / 100;
        self.detail_scale = if short {
            (self.detail_scale * 0.99).max(0.25)
        } else if spare {
            (self.detail_scale * 1.003).min(1.0)
        } else {
            self.detail_scale
        };
        self.started = true;
    }

    fn level_at_reference(&self, leaf: usize, distance: f32) -> f32 {
        let error_px = self.settings.lod_error_px;
        if error_px <= 0.0 {
            let d0 = (self.settings.lod_distance_m * self.detail_scale).max(0.1);
            return if distance <= d0 {
                0.0
            } else {
                (distance / d0).log2() + 1.0
            };
        }
        // A smaller budget allows a larger error.
        let target = error_px / self.detail_scale;
        let error = |l: usize| self.spacing[leaf][l] * self.focal_px / distance.max(0.05);
        let levels = self.spacing[leaf].len();
        if error(0) >= target {
            return 0.0;
        }
        for l in 0..levels.saturating_sub(1) {
            let (a, b) = (error(l), error(l + 1));
            if b > target {
                // Between l (fine enough) and l + 1 (too coarse): how far in log space.
                return l as f32 + ((target / a).ln() / (b / a).max(1.0001).ln()).clamp(0.0, 0.999);
            }
        }
        (levels - 1) as f32
    }
}

#[test]
fn shared_centre_lookup_preserves_quantized_values_and_edge_cases() {
    let s = stream(10_000);
    let mut shared = s.shared.clone();
    shared.means_mins = [-8.0, -0.0, -7.25];
    shared.means_maxs = [7.0, 0.0, 4.5];
    let table = make_centre_lookup(&shared).unwrap();
    for axis in 0..3 {
        for value in 0..=u16::MAX {
            let value = f32::from(value);
            assert_eq!(
                decoded_shared_centre(&shared, axis, value, Some(&table)).to_bits(),
                direct_shared_centre(&shared, axis, value).to_bits(),
                "axis {axis}, value {value}"
            );
        }
        for value in [
            -0.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::from_bits(0x7fc0_1234),
        ] {
            assert_eq!(
                decoded_shared_centre(&shared, axis, value, Some(&table)).to_bits(),
                direct_shared_centre(&shared, axis, value).to_bits()
            );
        }
    }
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bad = shared.clone();
        bad.means_mins[1] = invalid;
        assert!(make_centre_lookup(&bad).is_none());
    }
    let mut overflowing = shared.clone();
    overflowing.means_mins[0] = -f32::MAX;
    overflowing.means_maxs[0] = f32::MAX;
    assert!(make_centre_lookup(&overflowing).is_none());
}

#[test]
fn shared_centre_lookup_preserves_all_encoded_upload_bytes() {
    let s = stream(10_000);
    let remap = s.remaps[0].as_ref().unwrap();
    let count = 4099usize;
    let mut textures = BTreeMap::new();
    for (stream, seed) in [
        ("means_l", 1u32),
        ("means_u", 2),
        ("scales", 3),
        ("sh0", 4),
        ("sh_labels", 5),
    ] {
        let mut state = seed;
        let bytes = (0..count * 4)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        textures.insert(
            stream,
            sog::Texture {
                width: count as u32,
                height: 1,
                rgba: bytes,
            },
        );
    }
    let bundle = Bundle {
        meta: remap.meta.clone(),
        textures,
    };
    let table = make_centre_lookup(&s.shared).unwrap();
    for palette in [None, Some(0), Some(5)] {
        let range = Range {
            file: 0,
            offset: 3,
            count: (count - 3) as u32,
        };
        let direct = encode_upload(&bundle, remap, &s.shared, range, palette, None);
        let candidate = encode_upload(&bundle, remap, &s.shared, range, palette, Some(&table));
        assert_eq!(direct.scales, candidate.scales);
        assert_eq!(direct.sh0, candidate.sh0);
        assert_eq!(direct.labels, candidate.labels);
        for (a, b) in direct.centres.iter().zip(&candidate.centres) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
    }
}

#[test]
fn reserved_zero_sh_row_and_palette_labels_stay_distinct() {
    let mut stream = stream(10_000);
    let zero = stream.sh_zero;
    assert_eq!(
        stream.shared.sh_n_codebook[zero as usize].to_bits(),
        0.0f32.to_bits()
    );
    let row = zero_sh_centroid_row(zero);
    assert_eq!(row.len(), 64 * SH_COEFFS * 4);
    for texel in row.chunks_exact(4) {
        assert_eq!(texel[3], 255);
        for &component in &texel[..3] {
            assert_eq!(
                stream.shared.sh_n_codebook[component as usize].to_bits(),
                0.0f32.to_bits()
            );
        }
    }

    stream.remaps[0].as_mut().unwrap().meta.sh_n_count = 65_536;
    let remap = stream.remaps[0].as_ref().unwrap();
    let mut textures: BTreeMap<_, _> = ["means_l", "means_u", "scales", "sh0", "sh_labels"]
        .into_iter()
        .map(|name| {
            (
                name,
                sog::Texture {
                    width: 2,
                    height: 1,
                    rgba: vec![0; 8],
                },
            )
        })
        .collect();
    // A real source centroid 0 becomes label 64. Source centroid 65472 becomes
    // label 65536, whose low two bytes are zero but whose high byte is not.
    textures.get_mut("sh_labels").unwrap().rgba = vec![0, 0, 0, 255, 192, 255, 0, 255];
    let bundle = Bundle {
        meta: remap.meta.clone(),
        textures,
    };
    let range = Range {
        file: 0,
        offset: 0,
        count: 2,
    };
    let absent = encode_upload(&bundle, remap, &stream.shared, range, None, None);
    let present = encode_upload(&bundle, remap, &stream.shared, range, Some(0), None);
    assert_eq!(absent.labels, [0, 0, 0, 255, 0, 0, 0, 255]);
    assert_eq!(present.labels, [64, 0, 0, 255, 0, 0, 1, 255]);
}

/// Run only in a reserved CPU timing slot:
/// `cargo test --release --lib lod_upload_cpu_probe -- --ignored --nocapture --test-threads=1`.
/// Set LOD_UPLOAD_REVERSE=1 to reverse reference/candidate order. No graphics device is created.
#[test]
#[ignore]
fn lod_upload_cpu_probe() {
    use std::{hint::black_box, path::PathBuf, time::Instant};

    fn fingerprint(
        bundle: &Bundle,
        remap: &Remap,
        shared: &Meta,
        ranges: &[Range],
        lookup: Option<&[Vec<f32>; 3]>,
    ) -> u64 {
        let mut hash = 14_695_981_039_346_656_037u64;
        let mut add = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211);
            }
        };
        for &range in ranges {
            let encoded = encode_upload(bundle, remap, shared, range, Some(1), lookup);
            for centre in encoded.centres {
                for value in centre {
                    add(&value.to_bits().to_le_bytes());
                }
            }
            add(&encoded.scales);
            add(&encoded.sh0);
            add(&encoded.labels);
        }
        hash
    }

    let dir = std::env::var_os("LOD_UPLOAD_LEVEL")
        .map(PathBuf::from)
        .expect("set LOD_UPLOAD_LEVEL to a level directory before running this ignored benchmark");
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    let metas: Vec<Option<Vec<u8>>> = meta
        .filenames
        .iter()
        .map(|f| std::fs::read(dir.join(f)).ok())
        .collect();
    let file = meta
        .filenames
        .iter()
        .position(|f| f == "0_0/meta.json")
        .expect("0_0 chunk");
    let mut ranges: Vec<Range> = meta
        .leaves
        .iter()
        .filter_map(|leaf| leaf.lods.first().copied().flatten())
        .filter(|r| r.file == file && r.count > 0)
        .collect();
    ranges.sort_by_key(|r| r.offset);
    let stream = LevelStream::new(meta, &metas, StreamSettings::default()).unwrap();
    let bundle = sog::load_dir(&dir.join(stream.meta.dir_of(file).unwrap())).unwrap();
    let remap = stream.remaps[file].as_ref().unwrap();
    let shared = &stream.shared;
    let one = vec![*ranges.first().expect("LOD 0 ranges")];
    let mut batch = Vec::new();
    let mut count = 0u64;
    for range in ranges {
        count += u64::from(range.count);
        batch.push(range);
        if count >= 200_000 {
            break;
        }
    }
    assert!(count >= 200_000);
    let cases = [
        ("first-leaf", one.as_slice(), 20usize),
        ("200k-batch", batch.as_slice(), 1usize),
    ];
    let reverse = std::env::var_os("LOD_UPLOAD_REVERSE").is_some();
    for round in 0..4 {
        let start = Instant::now();
        let table = make_centre_lookup(shared).expect("finite shared range");
        let init_ns = start.elapsed().as_nanos();
        for (case, ranges, repetitions) in cases {
            let direct_hash = fingerprint(&bundle, remap, shared, ranges, None);
            let lookup_hash = fingerprint(&bundle, remap, shared, ranges, Some(&table));
            assert_eq!(direct_hash, lookup_hash, "{case} encoded data differs");
            let mut times = [0u128; 2];
            for index in if reverse { [1usize, 0] } else { [0usize, 1] } {
                let lookup = if index == 0 {
                    None
                } else {
                    Some(black_box(&table))
                };
                let start = Instant::now();
                for _ in 0..repetitions {
                    for &range in ranges {
                        black_box(encode_upload(
                            black_box(&bundle),
                            black_box(remap),
                            black_box(shared),
                            range,
                            Some(1),
                            black_box(lookup),
                        ));
                    }
                }
                times[index] = start.elapsed().as_nanos() / repetitions as u128;
            }
            let splats: u64 = ranges.iter().map(|r| u64::from(r.count)).sum();
            println!("{{\"case\":\"{case}\",\"round\":{round},\"ranges\":{},\"splats\":{splats},\"lookup_init_ns\":{init_ns},\"direct_ns\":{},\"lookup_ns\":{},\"fingerprint\":\"{direct_hash:016x}\"}}", ranges.len(), times[0], times[1]);
        }
    }
}

#[test]
fn selection_matches_reference_with_residents_and_boundaries() {
    let json = include_bytes!("../../tests/fixtures/lod-meta.json");
    let mut meta = LodMeta::parse(json).unwrap();
    // Cover equal priorities, absent intermediate levels and leaves with no available range.
    meta.leaves[1] = meta.leaves[0].clone();
    for (i, leaf) in meta.leaves.iter_mut().enumerate() {
        if i % 29 == 0 {
            leaf.lods[0] = None;
        }
        if i % 47 == 0 {
            leaf.lods[2] = None;
        }
        if i % 113 == 0 {
            leaf.lods.fill(None);
        }
    }
    let metas = vec![Some(meta_json()); meta.filenames.len()];
    let settings = StreamSettings::default();
    let mut reference = LevelStream::new(meta.clone(), &metas, settings).unwrap();
    let mut candidate = LevelStream::new(meta.clone(), &metas, settings).unwrap();
    let projection = crate::camera::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.05, 2000.0);
    let distances = [
        0.0, 7.999, 8.0, 8.001, 23.999, 24.0, 24.001, 47.999, 48.0, 48.001, 149.999, 150.0, 150.001,
    ];
    for frame in 0..1040 {
        let leaf = &meta.leaves[(frame / distances.len()) % meta.leaves.len()];
        let eye = [
            leaf.min[0] - distances[frame % distances.len()],
            (leaf.min[1] + leaf.max[1]) * 0.5,
            (leaf.min[2] + leaf.max[2]) * 0.5,
        ];
        let angle = (frame as f32 * 0.071).sin() * std::f32::consts::PI;
        let forward = [angle.cos(), 0.0, angle.sin()];
        let target = [eye[0] + forward[0], eye[1], eye[2] + forward[2]];
        let view = crate::camera::look_at(eye, target, [0.0, 1.0, 0.0]);
        let clip = crate::camera::mul(&projection, &view);
        let clip = (frame % 3 != 0).then_some(&clip);
        let capacity = [120_000, 500_000, 1_500_000, 3_000_000][(frame / 23) % 4];
        let error = [0.0, 0.2, 1.5, 3.0][(frame / 41) % 4];
        let focal = [350.0, 935.3074, 1600.0][(frame / 61) % 3];
        for stream in [&mut reference, &mut candidate] {
            stream.settings.capacity = capacity;
            stream.settings.lod_error_px = error;
            stream.settings.max_distance_m = [80.0, 150.0, 250.0][(frame / 71) % 3];
            stream.set_focal_px(focal);
            if frame % 31 == 0 {
                stream.squeezed = true;
            }
        }
        let now = frame as f32 / 60.0;
        reference.select_reference(eye, forward, clip, now);
        candidate.select(eye, forward, clip, now);
        assert_eq!(reference.frame, candidate.frame, "frame {frame}");
        assert_eq!(
            reference.now.to_bits(),
            candidate.now.to_bits(),
            "frame {frame}"
        );
        assert_eq!(reference.started, candidate.started, "frame {frame}");
        assert_eq!(reference.squeezed, candidate.squeezed, "frame {frame}");
        assert_eq!(
            reference.detail_scale.to_bits(),
            candidate.detail_scale.to_bits(),
            "frame {frame}"
        );
        assert_eq!(
            reference.distance_scale.to_bits(),
            candidate.distance_scale.to_bits(),
            "frame {frame}"
        );
        assert_eq!(
            reference.sharp_m.to_bits(),
            candidate.sharp_m.to_bits(),
            "frame {frame}"
        );
        for (i, (a, b)) in reference.leaves.iter().zip(&candidate.leaves).enumerate() {
            assert_eq!(a.want, b.want, "frame {frame}, leaf {i}");
            assert_eq!(a.visible, b.visible, "frame {frame}, leaf {i}");
            assert_eq!(
                a.distance.to_bits(),
                b.distance.to_bits(),
                "frame {frame}, leaf {i}"
            );
            assert_eq!(
                a.coarsen_since.map(f32::to_bits),
                b.coarsen_since.map(f32::to_bits),
                "frame {frame}, leaf {i}"
            );
        }
        if frame % 7 == 0 {
            let a = reference.requests();
            let b = candidate.requests();
            assert_eq!(a, b, "request order at frame {frame}");
            for (file, _) in a {
                reference.failed(file);
                candidate.failed(file);
            }
        }
        // CPU-only simulated arrivals let resident LODs catch up at different times, exercising hysteresis and
        // sticky prioritization. They do not represent GPU upload throughput or rendered pixels.
        for i in 0..meta.leaves.len() {
            if (i + frame) % 7 == 0 {
                let resident = candidate.leaves[i].want.map(|lod| Resident {
                    lod,
                    start: i as u32 * 100,
                    count: meta.leaves[i].lods[lod].unwrap().count,
                    palette: None,
                    born: now,
                });
                reference.leaves[i].have = resident;
                candidate.leaves[i].have = resident;
            }
        }
    }
}

#[test]
fn palette_texture_preserves_reachable_slots_without_unused_rows() {
    for sh_files in [0usize, 1, 2, 11, 15, 20] {
        let files = sh_files.max(2);
        let mut meta = tree(6);
        meta.filenames = (0..files).map(|i| format!("{i}/meta.json")).collect();
        let metas: Vec<_> = (0..files)
            .map(|i| {
                let mut value: serde_json::Value = serde_json::from_slice(&meta_json()).unwrap();
                if i >= sh_files {
                    value.as_object_mut().unwrap().remove("shN");
                }
                Some(serde_json::to_vec(&value).unwrap())
            })
            .collect();
        let stream = LevelStream::new(meta, &metas, StreamSettings::default()).unwrap();
        for limit in [1024, 2048, 8192, 16384, 32768] {
            let rows = stream.palette_texture_rows(limit);
            let old_slots = (limit.min(POOL_CENTROID_ROWS) - 1) / PALETTE_ROWS;
            assert_eq!((rows - 1) / PALETTE_ROWS, old_slots.min(sh_files as u32));
            assert_eq!((rows - 1) % PALETTE_ROWS, 0);
            assert!(rows <= limit && rows <= POOL_CENTROID_ROWS);
        }
    }
}

#[test]
fn near_leaves_want_lod_0_and_far_ones_coarser() {
    let mut s = stream(10_000);
    s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
    let wants: Vec<Option<usize>> = s.leaves.iter().map(|l| l.want).collect();
    assert_eq!(
        wants,
        [Some(0), Some(0), Some(1), Some(1), Some(1), Some(1)]
    );
}

#[test]
fn a_small_pool_refines_the_nearest_leaves_first() {
    let mut s = stream(150);
    s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
    let total: u32 = s
        .leaves
        .iter()
        .zip(&s.meta.leaves)
        .map(|(st, l)| l.lods[st.want.unwrap()].unwrap().count)
        .sum();
    assert!(
        total * 10 <= 150 * 9,
        "selection of {total} splats fits 150 slots"
    );
    assert_eq!(
        s.leaves[0].want,
        Some(1),
        "nothing fits a 100-splat refinement after the coarse base of 60"
    );
    let mut roomy = stream(400);
    roomy.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
    let wants: Vec<Option<usize>> = roomy.leaves.iter().map(|l| l.want).collect();
    assert_eq!(
        wants[..3],
        [Some(0), Some(0), Some(1)],
        "the two nearest get LOD 0, the rest stay coarse: {wants:?}"
    );
}

/// A camera at the origin looking along +x (column-major projection · view, 90° field of view).
fn looking_along_x() -> [f32; 16] {
    let view = crate::camera::look_at([0.0, 0.5, 0.5], [1.0, 0.5, 0.5], [0.0, 1.0, 0.0]);
    crate::camera::mul(
        &crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0),
        &view,
    )
}

#[test]
fn leaves_out_of_view_go_coarser_but_not_those_near_the_camera() {
    // Leaves at x = 0..55; the camera stands at x = 27 looking along +x. With LOD 0 out to 4 m, leaves within 8 m
    // count as in view whichever way the camera looks.
    let mut s = stream(10_000);
    s.settings.lod_distance_m = 4.0;
    s.settings.behind_bias = 1;
    let view = crate::camera::look_at([27.0, 0.5, 0.5], [28.0, 0.5, 0.5], [0.0, 1.0, 0.0]);
    let clip = crate::camera::mul(
        &crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0),
        &view,
    );
    s.select([27.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&clip), 0.0);
    let visible: Vec<bool> = s.leaves.iter().map(|l| l.visible).collect();
    assert_eq!(
        visible,
        [false, false, true, true, true, true],
        "behind the camera, only the leaf 2 m away counts"
    );
    assert_eq!(
        s.leaves[2].want,
        Some(0),
        "the near leaf behind the camera keeps LOD 0"
    );
}

#[test]
fn going_coarser_waits_the_keep_delay() {
    let mut s = stream(10_000);
    s.settings.keep_s = 1.0;
    s.select(
        [0.0, 0.5, 0.5],
        [1.0, 0.0, 0.0],
        Some(&looking_along_x()),
        0.0,
    );
    assert_eq!(s.leaves[1].want, Some(0));
    s.leaves[1].have = Some(Resident {
        lod: 0,
        start: 0,
        count: 100,
        palette: None,
        born: 0.0,
    });
    // Backing away to 30 m from it wants it a level coarser...
    let view = crate::camera::look_at([-20.0, 0.5, 0.5], [-19.0, 0.5, 0.5], [0.0, 1.0, 0.0]);
    let clip = crate::camera::mul(
        &crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0),
        &view,
    );
    s.select([-20.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&clip), 0.5);
    assert_eq!(s.leaves[1].want, Some(0), "...but not for the first second");
    s.select([-20.0, 0.5, 0.5], [1.0, 0.0, 0.0], Some(&clip), 1.6);
    assert_eq!(s.leaves[1].want, Some(1), "after it");
    // Coming back resets the delay.
    s.select(
        [0.0, 0.5, 0.5],
        [1.0, 0.0, 0.0],
        Some(&looking_along_x()),
        1.7,
    );
    assert_eq!(s.leaves[1].coarsen_since, None);
}

#[test]
fn an_out_of_view_leaf_does_not_flip_between_two_levels() {
    // A leaf out of view just short of its band edge (22.4 m from a 12 m LOD distance is level 1.9) must stay
    // put, frame after frame, rather than flip between levels 2 and 3.
    let mut s = stream(10_000);
    s.settings.behind_bias = 1;
    s.settings.keep_s = 0.0;
    let back = crate::camera::look_at([-2.4, 0.5, 0.5], [-3.4, 0.5, 0.5], [0.0, 1.0, 0.0]);
    let clip = crate::camera::mul(
        &crate::camera::perspective(90f32.to_radians(), 1.0, 0.1, 1000.0),
        &back,
    );
    let mut wants = Vec::new();
    for frame in 0..6 {
        s.select(
            [-2.4, 0.5, 0.5],
            [-1.0, 0.0, 0.0],
            Some(&clip),
            frame as f32,
        );
        let want = s.leaves[2].want.unwrap();
        s.leaves[2].have = Some(Resident {
            lod: want,
            start: 0,
            count: 10,
            palette: None,
            born: 0.0,
        });
        wants.push(want);
    }
    assert!(
        wants.windows(2).all(|w| w[0] == w[1]),
        "the leaf 22 m behind the camera settles: {wants:?}"
    );
}

#[test]
fn by_screen_space_error_a_denser_leaf_goes_coarser_at_the_same_distance() {
    // Two leaves the same size and distance; the second holds four times the splats at every LOD, so its splats
    // sit half as far apart and it can drop two levels for the same error.
    let mut meta = tree(2);
    meta.leaves[1].min = meta.leaves[0].min;
    meta.leaves[1].max = meta.leaves[0].max;
    for l in 0..2 {
        let r = meta.leaves[1].lods[l].as_mut().unwrap();
        r.count *= 4;
    }
    let settings = StreamSettings {
        capacity: 100_000,
        lod_error_px: 1.0,
        behind_bias: 0,
        ..StreamSettings::default()
    };
    let mut s = LevelStream::new(meta, &[Some(meta_json()), Some(meta_json())], settings).unwrap();
    s.set_focal_px(1000.0);
    // Leaf 0: 100 splats on 5 × 1 m, 0.22 m apart; at 150 m that is 1.5 px: LOD 0 is already too coarse.
    // Leaf 1: 0.11 m apart, 0.75 px: LOD 0 fits, LOD 1 (0.35 m, 2.4 px) does not.
    assert!(s.level_at(0, 150.0) < 0.5 && s.level_at(1, 150.0) >= 0.0);
    assert!(
        s.level_at(1, 300.0) > s.level_at(0, 300.0),
        "the denser leaf is allowed a coarser level"
    );
    // Far enough away, both reach their coarsest.
    assert_eq!(s.level_at(0, 10_000.0), 1.0);
}

#[test]
fn a_leaf_at_the_edge_of_the_budget_keeps_its_detail() {
    // Room for one refinement: leaves 1 and 2 are 10 and 20 m away... the camera moves so that leaf 2 is a little
    // nearer than leaf 1, but leaf 1 already has LOD 0 and keeps it.
    let mut s = stream(200);
    s.settings.lod_distance_m = 25.0;
    s.select([12.0, 0.5, 0.5], [1.0, 0.0, 0.0], None, 0.0);
    let first: Vec<usize> = (0..6).filter(|&i| s.leaves[i].want == Some(0)).collect();
    assert_eq!(first, [1], "the nearest leaf gets the one refinement");
    s.leaves[1].have = Some(Resident {
        lod: 0,
        start: 0,
        count: 100,
        palette: None,
        born: 0.0,
    });
    s.select([16.0, 0.5, 0.5], [1.0, 0.0, 0.0], None, 0.1);
    assert_eq!(
        s.leaves[1].want,
        Some(0),
        "1 m farther than leaf 2 now, but settled: it keeps LOD 0"
    );
    assert_eq!(s.leaves[2].want, Some(1));
}

#[test]
fn new_and_old_ranges_crossfade_without_a_brightness_spike() {
    let mut s = stream(10_000);
    s.settings.fade_s = 0.4;
    s.select([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], None, 1.0);
    s.leaves[0].have = Some(Resident {
        lod: 0,
        start: 100,
        count: 100,
        palette: None,
        born: 0.8,
    });
    s.leaves[1].have = Some(Resident {
        lod: 0,
        start: 300,
        count: 100,
        palette: None,
        born: 0.0,
    });
    s.fading_out.push((
        Resident {
            lod: 1,
            start: 0,
            count: 10,
            palette: None,
            born: 0.0,
        },
        0.8,
    ));
    let fades = s.fades();
    assert_eq!(fades.len(), 2, "the settled leaf is not fading");
    assert_eq!((fades[0].0, fades[0].1), (0, 10));
    assert!((fades[0].2 - 0.5).abs() < 1e-5);
    assert!((fades[1].2 - 0.5).abs() < 1e-5);
    assert!((fades[0].2 + fades[1].2 - 1.0).abs() < 1e-5);
    s.settings.fade_s = 0.0;
    assert!(s.fades().is_empty());
}

#[test]
fn empty_leaf_placeholders_do_not_exceed_the_selected_detail() {
    let s = stream(10_000);
    let lods = &s.meta.leaves[0].lods;
    assert_eq!(
        loaded_fallback(lods, 1, |file| file == 0),
        None,
        "a fine chunk should not fill the pool when the selected LOD is coarse"
    );
    assert_eq!(
        loaded_fallback(lods, 0, |file| file == 1),
        Some(1),
        "a loaded coarser chunk can cover a leaf while its detail loads"
    );
    assert_eq!(loaded_fallback(lods, 0, |_| false), None);
}

#[test]
fn transitions_wait_for_the_previous_fade_and_retirement_keeps_its_opacity() {
    let resident = Resident {
        lod: 0,
        start: 0,
        count: 100,
        palette: None,
        born: 10.0,
    };
    assert!(!resident_is_settled(resident, 10.1, 0.4));
    assert!(resident_is_settled(resident, 10.5, 0.4));
    assert!(resident_is_settled(resident, 10.0, 0.0));

    let now = 10.1;
    let fade = 0.4;
    let before = (now - resident.born) / fade;
    let after = 1.0 - (now - retirement_start(resident, now, fade)) / fade;
    assert!(
        (before - after).abs() < 1e-5,
        "retirement must not flash brighter"
    );
    assert!((retirement_start(resident, 11.0, fade) - 11.0).abs() < 1e-5);
}

#[test]
fn chunk_requests_never_reserve_more_than_idle_decoders() {
    let mut s = stream(10_000);
    s.settings.max_in_flight = 4;
    s.settings.near_lod0_m = 25.0;
    s.select([0.0; 3], [1.0, 0.0, 0.0], None, 0.0);
    assert!(s.requests_up_to(0).is_empty());
    let first = s.requests_up_to(1);
    assert_eq!(first.len(), 1);
    let second = s.requests_up_to(1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].0, second[0].0);
    assert!(s.requests_up_to(4).is_empty());
}

#[test]
fn nearby_visible_regions_request_fine_detail_first_without_overriding_manual_lod() {
    let mut s = stream(10_000);
    s.settings.near_lod0_m = 25.0;
    s.settings.lod_distance_m = 1.0;
    s.select([0.0; 3], [1.0, 0.0, 0.0], None, 0.0);
    assert_eq!(s.leaves[2].want, Some(0));
    assert_eq!(s.leaves[4].want, Some(1));
    let requests = s.requests();
    assert_eq!(
        requests[0].0, 0,
        "nearby empty regions request their fine file first"
    );
    s.set_fixed_lod(Some(1)).unwrap();
    s.select([0.0; 3], [1.0, 0.0, 0.0], None, 1.0);
    assert_eq!(
        s.leaves[0].want,
        Some(1),
        "manual coarse mode remains authoritative"
    );
}

#[test]
fn leaves_past_the_draw_distance_are_not_drawn() {
    let mut s = stream(10_000);
    s.settings.max_distance_m = 25.0;
    s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
    let shown = s.leaves.iter().filter(|l| l.want.is_some()).count();
    assert_eq!(
        shown, 3,
        "leaves at 0, 10 and 20 m are within 25 m; 30 m and beyond are hidden"
    );
}

#[test]
fn eviction_keeps_cached_and_needed_chunks_at_cache_boundary() {
    let mut s = stream(10_000);
    s.settings.cache_chunks = 2;
    for file in 0..2 {
        let bundle = Bundle {
            meta: s.remaps[file].as_ref().unwrap().meta.clone(),
            textures: BTreeMap::new(),
        };
        s.chunks.insert(
            file,
            Chunk {
                bundle,
                last_used: file as u64,
            },
        );
    }
    s.evict();
    assert_eq!(s.chunks.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
    // Above the boundary, a pending wanted chunk is protected and the oldest idle chunk goes.
    s.settings.cache_chunks = 0;
    for leaf in &mut s.leaves {
        leaf.want = None;
    }
    s.leaves[0].want = Some(0);
    s.evict();
    assert_eq!(s.chunks.keys().copied().collect::<Vec<_>>(), vec![0]);
}

#[test]
fn layout_diagnostics_are_opt_in_and_separate_final_mapping_from_allocation_order() {
    let mut first = stream(10_000);
    assert!(first.layout_diagnostics().is_none());
    assert!(first.stats().json().get("layout_diagnostics").is_none());
    first.set_layout_diagnostics(true);
    first.frame = 5;
    first.now = 0.25;

    let a = first.meta.leaves[0].lods[0].unwrap();
    let b = first.meta.leaves[1].lods[1].unwrap();
    first.leaves[0].have = Some(Resident {
        lod: 0,
        start: 120,
        count: a.count,
        palette: Some(2),
        born: 0.0,
    });
    first.leaves[1].have = Some(Resident {
        lod: 1,
        start: 20,
        count: b.count,
        palette: None,
        born: 0.0,
    });
    first.fading_out.push((
        Resident {
            lod: 1,
            start: 500,
            count: b.count,
            palette: None,
            born: 0.0,
        },
        0.2,
    ));
    first.record_layout_allocation(0, 0, a, 120, Some(2));
    first.record_layout_allocation(1, 1, b, 20, None);
    assert!(first.stats().json().get("layout_diagnostics").is_none());

    let mut second = stream(10_000);
    second.set_layout_diagnostics(true);
    second.frame = 5;
    second.now = 0.25;
    let a = second.meta.leaves[0].lods[0].unwrap();
    let b = second.meta.leaves[1].lods[1].unwrap();
    second.leaves[0].have = Some(Resident {
        lod: 0,
        start: 120,
        count: a.count,
        palette: Some(2),
        born: 0.0,
    });
    second.leaves[1].have = Some(Resident {
        lod: 1,
        start: 20,
        count: b.count,
        palette: None,
        born: 0.0,
    });
    second.record_layout_allocation(1, 1, b, 20, None);
    second.record_layout_allocation(0, 0, a, 120, Some(2));
    assert!(
        second.stats().json().get("layout_diagnostics").is_none(),
        "enabled diagnostics do not leak into routine stats"
    );

    let first = first.layout_diagnostics().unwrap();
    let second = second.layout_diagnostics().unwrap();
    assert_eq!(first.resident_fingerprint, second.resident_fingerprint);
    assert_eq!(first.resident_ranges, second.resident_ranges);
    assert_eq!(
        first.resident_ranges.len(),
        2,
        "fading-out ranges are outside the current leaf mapping snapshot"
    );
    assert_ne!(first.allocation_fingerprint, second.allocation_fingerprint);
    assert_ne!(first.allocation_history, second.allocation_history);
    assert_eq!(first.allocation_count, 2);
    assert_eq!(first.allocation_history[0].leaf, 0);
    assert_eq!(first.allocation_history[1].leaf, 1);
    assert!(first.json().get("resident_ranges").is_some());
    assert_eq!(
        first.json()["resident_mapping_scope"],
        "current_leaf_residents_excluding_fading_out"
    );
}

#[test]
fn empty_leaves_request_their_coarsest_file_first() {
    let mut s = stream(10_000);
    s.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 0.0);
    let first = s.requests();
    assert_eq!(first[0].0, 1, "the coarse file comes first: {first:?}");
}

#[test]
fn the_allocator_reuses_and_merges_freed_ranges() {
    let mut s = stream(1000);
    let a = s.allocate(100).unwrap();
    let b = s.allocate(200).unwrap();
    let c = s.allocate(50).unwrap();
    assert_eq!((a, b, c), (0, 100, 300));
    assert_eq!(s.high_water(), 350);
    s.release(b, 200);
    assert_eq!(s.allocate(150), Some(100), "first fit reuses the hole");
    s.release(100, 150);
    s.release(a, 100);
    s.release(c, 50);
    assert_eq!(s.free, vec![(0, 1000)], "everything merges back");
    assert_eq!(s.high_water(), 0);
    s.take_exact(200, 100);
    assert_eq!(
        s.free,
        vec![(0, 200), (300, 700)],
        "an exact range comes out of the middle"
    );
    assert_eq!(s.allocate(250), Some(300));
}

#[test]
fn manual_lod_changes_targets_and_auto_can_be_restored() {
    let mut stream = stream(10_000);
    let eye = [0.0, 0.0, 0.0];
    let forward = [1.0, 0.0, 0.0];
    assert!(stream.set_fixed_lod(Some(2)).is_err());
    stream.set_fixed_lod(Some(1)).unwrap();
    stream.select(eye, forward, None, 1.0);
    assert!(stream.leaves.iter().all(|leaf| leaf.want == Some(1)));
    assert!(stream.prefetch().is_empty());
    stream.set_fixed_lod(Some(0)).unwrap();
    stream.select(eye, forward, None, 2.0);
    assert!(stream.leaves.iter().all(|leaf| leaf.want == Some(0)));
    stream.set_fixed_lod(None).unwrap();
    stream.select(eye, forward, None, 3.0);
    assert_eq!(stream.leaves[0].want, Some(0));
    assert_eq!(stream.leaves[5].want, Some(1));
}

#[test]
fn manual_lod_respects_the_splat_pool_budget() {
    let mut stream = stream(120);
    stream.set_fixed_lod(Some(0)).unwrap();
    stream.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 1.0);
    assert!(stream.leaves.iter().any(|leaf| leaf.want == Some(1)));
    let selected: u32 = stream
        .leaves
        .iter()
        .zip(&stream.meta.leaves)
        .filter_map(|(state, leaf)| state.want.and_then(|lod| leaf.lods[lod]))
        .map(|range| range.count)
        .sum();
    assert!(selected <= stream.settings.capacity * SELECT_SHARE as u32 / 100);
}

#[test]
fn manual_lod_uses_a_level_that_exists_for_each_leaf() {
    let mut meta = tree(6);
    meta.leaves[0].lods[1] = None;
    let mut stream = LevelStream::new(
        meta,
        &[Some(meta_json()), Some(meta_json())],
        StreamSettings::default(),
    )
    .unwrap();
    stream.set_fixed_lod(Some(1)).unwrap();
    stream.select([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], None, 1.0);
    assert_eq!(stream.leaves[0].want, Some(0));
    assert_eq!(stream.leaves[1].want, Some(1));
}

#[test]
fn forward_ellipse_covers_ahead_more_than_behind_and_turns_with_the_camera() {
    let intersects = |point: [f32; 3], direction: [f32; 3], margin| {
        forward_ellipse_intersects([0.0; 3], direction, point, point, 100.0, 0.3, 0.55, margin)
    };
    assert!(intersects([99.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0));
    assert!(!intersects([101.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0));
    assert!(intersects([-29.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0));
    assert!(!intersects([-31.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0));
    assert!(intersects([35.0, 0.0, 54.0], [1.0, 0.0, 0.0], 1.0));
    assert!(!intersects([35.0, 0.0, 56.0], [1.0, 0.0, 0.0], 1.0));
    assert!(intersects([0.0, 0.0, 99.0], [0.0, 0.0, 1.0], 1.0));
    assert!(intersects([105.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.15));
    // A box intersecting the edge is retained even when no corner lies in the oval.
    assert!(forward_ellipse_intersects(
        [0.0; 3],
        [1.0, 0.0, 0.0],
        [34.0, -1.0, -100.0],
        [36.0, 1.0, 100.0],
        100.0,
        0.3,
        0.55,
        1.0
    ));
}

#[test]
fn forward_region_retains_resident_leaves_briefly_when_turning() {
    let mut stream = stream(10_000);
    stream.settings.max_distance_m = 25.0;
    stream.settings.draw_region = DrawRegion::ForwardEllipse {
        rear_fraction: 0.3,
        side_fraction: 0.55,
    };
    stream.select([0.0; 3], [1.0, 0.0, 0.0], None, 0.0);
    assert!(stream.leaves[2].want.is_some());
    assert!(stream.leaves[3].want.is_none());
    stream.leaves[2].have = Some(Resident {
        lod: 1,
        start: 0,
        count: 10,
        palette: None,
        born: 0.0,
    });
    stream.select([0.0; 3], [-1.0, 0.0, 0.0], None, 0.1);
    assert!(
        stream.leaves[2].want.is_some(),
        "an abrupt turn must not immediately remove resident detail"
    );
    stream.select([0.0; 3], [-1.0, 0.0, 0.0], None, 0.7);
    assert!(stream.leaves[2].want.is_some());
    stream.select([0.0; 3], [-1.0, 0.0, 0.0], None, 1.4);
    assert!(
        stream.leaves[2].want.is_none(),
        "an out-of-region leaf can retire after the hold time"
    );
}

#[test]
fn octree_candidates_include_all_intersecting_and_previously_active_regions() {
    use std::collections::BTreeSet;
    let meta = tree(200);
    let mut index = SpatialOctree::new(&meta.leaves);
    assert!(
        index.node_count() > 1,
        "spatial index must actually subdivide"
    );
    index.set_active(vec![199]);
    for direction in [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
        let eye = [250.0, 0.5, 0.5];
        let mut ids = Vec::new();
        index.forward_candidates(eye, direction, 60.0, 0.3, 0.55, &mut ids);
        let candidates: BTreeSet<_> = ids.into_iter().collect();
        assert!(
            candidates.contains(&199),
            "previously active regions must be revisited for retirement"
        );
        for (i, leaf) in meta.leaves.iter().enumerate() {
            if forward_ellipse_intersects(eye, direction, leaf.min, leaf.max, 60.0, 0.3, 0.55, 1.2)
            {
                assert!(
                    candidates.contains(&i),
                    "octree omitted intersecting region {i}"
                );
            }
        }
        assert!(
            candidates.len() < meta.leaves.len(),
            "octree should prune distant regions"
        );
    }
}

#[test]
fn far_offscreen_regions_yield_budget_after_a_retention_delay() {
    let mut meta = tree(3);
    meta.leaves[0].min[0] = -30.0;
    meta.leaves[0].max[0] = -25.0;
    let mut stream = LevelStream::new(
        meta,
        &[Some(meta_json()), Some(meta_json())],
        StreamSettings {
            capacity: 10_000,
            offscreen_keep_m: 8.0,
            any_view_m: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    stream.select([0.0; 3], [1.0, 0.0, 0.0], None, 0.0);
    assert!(
        stream.leaves[0].want.is_none(),
        "far regions behind the player should not consume the pool"
    );
    assert!(stream.leaves[1].want.is_some());
    stream.leaves[1].have = Some(Resident {
        lod: 1,
        start: 0,
        count: 10,
        palette: None,
        born: 0.0,
    });
    stream.select([0.0; 3], [-1.0, 0.0, 0.0], None, 0.1);
    assert!(
        stream.leaves[1].want.is_some(),
        "a quick turn should retain previously visible detail"
    );
    stream.select([0.0; 3], [-1.0, 0.0, 0.0], None, 1.4);
    assert!(
        stream.leaves[1].want.is_none(),
        "offscreen detail retires after the delay"
    );
}

#[test]
fn center_of_view_wins_lod_zero_under_a_tight_budget() {
    let mut meta = tree(2);
    meta.leaves[0].min = [-1.0, -1.0, -21.0];
    meta.leaves[0].max = [1.0, 1.0, -19.0];
    meta.leaves[1].min = [6.0, -1.0, -11.0];
    meta.leaves[1].max = [8.0, 1.0, -9.0];
    let settings = StreamSettings {
        capacity: 150,
        near_lod0_m: 50.0,
        center_lod0_m: 100.0,
        ..Default::default()
    };
    let mut stream =
        LevelStream::new(meta, &[Some(meta_json()), Some(meta_json())], settings).unwrap();
    let eye = [0.0; 3];
    let forward = [0.0, 0.0, -1.0];
    let projection = crate::camera::perspective(90.0f32.to_radians(), 1.0, 0.05, 200.0);
    let view = crate::camera::look_at(eye, forward, [0.0, 1.0, 0.0]);
    let clip = crate::camera::mul(&projection, &view);
    stream.select(eye, forward, Some(&clip), 0.0);
    assert!(stream.leaves[0].centered);
    assert!(!stream.leaves[1].centered);
    assert_eq!(
        stream.leaves[0].want,
        Some(0),
        "center detail gets the first refinement"
    );
    assert_eq!(
        stream.leaves[1].want,
        Some(1),
        "a closer peripheral region yields under pressure"
    );
    assert_eq!(
        stream.requests()[0].0,
        0,
        "center LOD 0 is requested before peripheral coarse data"
    );
    stream.set_fixed_lod(Some(1)).unwrap();
    stream.select(eye, forward, Some(&clip), 1.0);
    assert_eq!(
        stream.leaves[0].want,
        Some(1),
        "manual LOD remains authoritative"
    );
}

#[test]
fn compaction_selects_the_cheapest_holes_and_preserves_pool_coverage() {
    let mut s = stream(340);
    s.free = vec![(0, 50), (100, 80), (240, 100)];
    let resident = |start, count| Resident {
        lod: 0,
        start,
        count,
        palette: None,
        born: 0.0,
    };
    s.leaves[0].have = Some(resident(50, 50));
    s.leaves[1].have = Some(resident(180, 60));
    assert!(s.check_pool().is_ok());
    let (leaf, hole, size) = s
        .compaction_move(150)
        .expect("fragmented space can be joined");
    assert_eq!((leaf, hole, size), (1, 100, 80));
    let mut moved = s.leaves[leaf].have.unwrap();
    s.take_exact(hole, size);
    s.release(hole + moved.count, size);
    moved.start = hole;
    s.leaves[leaf].have = Some(moved);
    assert_eq!(s.free, vec![(0, 50), (160, 180)]);
    assert!(s.check_pool().is_ok());
    assert_eq!(s.allocate(150), Some(160));
}

#[test]
fn lower_band_palettes_keep_coefficients_and_pad_with_exact_shared_zero() {
    let remap = std::array::from_fn(|i| (i as u8).wrapping_add(17));
    for coefficients in [3usize, 8, 15] {
        let width = 64 * coefficients;
        let mut rgba = vec![0; width * 2 * 4];
        for (i, texel) in rgba.chunks_exact_mut(4).enumerate() {
            texel.copy_from_slice(&[(i % 199) as u8, 22, 33, 200]);
        }
        let texture = sog::Texture {
            width: width as u32,
            height: 2,
            rgba,
        };
        let encoded = encode_sh_palette(&texture, &remap, 111).unwrap();
        assert_eq!(encoded.len(), 2 * 64 * 15 * 4);
        for row in 0..2 {
            for centroid in 0..64 {
                for coefficient in 0..15 {
                    let out = ((row * 64 + centroid) * 15 + coefficient) * 4;
                    if coefficient < coefficients {
                        let input = ((row * 64 + centroid) * coefficients + coefficient) * 4;
                        assert_eq!(
                            &encoded[out..out + 4],
                            &[
                                remap[texture.rgba[input] as usize],
                                remap[22],
                                remap[33],
                                200,
                            ]
                        );
                    } else {
                        assert_eq!(&encoded[out..out + 4], &[111, 111, 111, 255]);
                    }
                }
            }
        }
    }
    assert!(encode_sh_palette(
        &sog::Texture {
            width: 256,
            height: 1,
            rgba: vec![0; 1024]
        },
        &remap,
        0
    )
    .is_none());
    assert!(encode_sh_palette(
        &sog::Texture {
            width: 192,
            height: 1,
            rgba: vec![0; 10]
        },
        &remap,
        0
    )
    .is_none());
}
