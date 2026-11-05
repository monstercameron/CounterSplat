//! Level-of-detail streaming for a level's splats: the finest detail near the camera, coarser further away, and only
//! the chunk files that selection needs in memory.
//!
//! A streamed level (`lod-meta.json`) describes spatial leaves with one or more detail levels (LOD 0 is finest).
//! The browser indexes their bounds in an octree and queries a forward-biased oval; previously active regions remain
//! tracked while they leave the volume. Each frame selection prioritizes centered, visible, and nearby regions within
//! a fixed GPU budget. Manual LOD requests can override automatic screen-space selection.
//!
//! The host fetches and decodes the requested chunk files. Loaded ranges enter a shared GPU pool incrementally, and
//! replacements crossfade. Retention and hysteresis limit churn; when free pool space is fragmented, resident ranges
//! move incrementally to make room without discarding them. Unneeded decoded chunks are evicted from a bounded cache.
//!
//! The shared format: positions re-quantised to the union of every chunk's range (16 bits per axis in log space, as
//! SOG stores them), codebooks merged over every chunk, and SH centroids in palette slots of the pool's centroid
//! texture. Only leaves finer than `sh_lods` get a palette (view-dependent colour matters close up); the rest point at
//! a reserved all-zero centroid.

use crate::lod::{LodMeta, Range};
use crate::scene::{GsplatScene, POOL_CENTROID_ROWS};
use crate::sog::{self, Bundle, Meta};
use std::collections::{BTreeMap, BTreeSet};

mod selection;
mod simd;
mod spatial;
mod types;
mod upload;

#[cfg(test)]
use selection::forward_ellipse_intersects;
use selection::{
    box_in_frustum, coarsest_available, effective_distance, frustum_planes, leaf_count,
    level_at_values, ForwardEllipseQuery,
};
use spatial::SpatialOctree;
use types::{hash_allocation_event, hash_resident_layout};
pub use types::{
    DebugLeaf, DrawRegion, LayoutAllocationEvent, ResidentRangeLayout, StreamLayoutDiagnostics,
    StreamSettings, StreamStats,
};
#[cfg(test)]
use upload::encode_upload;
#[cfg(test)]
use upload::{decoded_shared_centre, direct_shared_centre, make_centre_lookup};
use upload::{encode_upload_into, CentreLookup, EncodedUpload};

// Retain at most roughly 6 MiB of staging data for ordinary leaf uploads.
// Unusually large leaves use temporary buffers instead.
const UPLOAD_SCRATCH_SPLATS: usize = 262_144;

/// SH centroids per palette (one chunk's palette), and rows of the centroid texture per palette.
const PALETTE_CENTROIDS: u32 = 65536;
const PALETTE_ROWS: u32 = PALETTE_CENTROIDS / 64;
const SH_COEFFS: usize = 15;
const LAYOUT_HASH_OFFSET: u64 = 0xcbf29ce484222325;
const LAYOUT_HASH_PRIME: u64 = 0x100000001b3;
const MAX_LAYOUT_ALLOCATION_EVENTS: usize = 4096;

/// Every RGB channel in the reserved row decodes through the shared codebook to +0.0.
fn zero_sh_centroid_row(sh_zero: u8) -> Vec<u8> {
    [sh_zero, sh_zero, sh_zero, 255].repeat(64 * SH_COEFFS)
}

/// Expand a chunk's 1-, 2-, or 3-band palette into the pool's 3-band layout.
/// Unprovided coefficients use the shared codebook's exact zero, not the
/// chunk's nearest-to-zero value, so lower-band assets keep their intended color.
fn encode_sh_palette(centroids: &sog::Texture, remap: &[u8; 256], zero: u8) -> Option<Vec<u8>> {
    let source_coeffs = match centroids.width {
        192 => 3usize,
        512 => 8,
        960 => 15,
        _ => return None,
    };
    let rows = centroids.height.min(PALETTE_ROWS) as usize;
    let source_row_bytes = centroids.width as usize * 4;
    let source = centroids.rgba.get(..rows.checked_mul(source_row_bytes)?)?;
    if source_coeffs == SH_COEFFS {
        return Some(
            source
                .chunks_exact(4)
                .flat_map(|t| {
                    [
                        remap[t[0] as usize],
                        remap[t[1] as usize],
                        remap[t[2] as usize],
                        t[3],
                    ]
                })
                .collect(),
        );
    }
    let mut data = zero_sh_centroid_row(zero).repeat(rows);
    for (source_row, destination_row) in source
        .chunks_exact(source_row_bytes)
        .zip(data.chunks_exact_mut(64 * SH_COEFFS * 4))
    {
        for (input, output) in source_row
            .chunks_exact(source_coeffs * 4)
            .zip(destination_row.chunks_exact_mut(SH_COEFFS * 4))
        {
            for (texel, mapped) in input.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
                mapped.copy_from_slice(&[
                    remap[texel[0] as usize],
                    remap[texel[1] as usize],
                    remap[texel[2] as usize],
                    texel[3],
                ]);
            }
        }
    }
    Some(data)
}

#[derive(Debug, Clone, Copy)]
struct Resident {
    lod: usize,
    start: u32,
    count: u32,
    palette: Option<usize>,
    /// When it was uploaded: it fades in over `fade_s` from then.
    born: f32,
}

#[derive(Debug, Clone, Copy)]
struct LeafState {
    /// The LOD to show; `None` past the draw distance.
    want: Option<usize>,
    have: Option<Resident>,
    distance: f32,
    /// In the view frustum (always true without one).
    visible: bool,
    /// Intersects the center-of-view region and receives priority for fine detail.
    centered: bool,
    /// Since when the selection has wanted this leaf coarser than it is.
    coarsen_since: Option<f32>,
    /// First time a resident region left the larger retention footprint.
    outside_since: Option<f32>,
    /// First time a far resident region left the widened view.
    offscreen_since: Option<f32>,
}

/// One in-range leaf competing for the pool. Keeping its priority here avoids recomputing it in every comparison.
struct SelectionCandidate {
    leaf: usize,
    target: usize,
    coarse: usize,
    priority: f32,
    visible: bool,
    centered: bool,
}

struct Chunk {
    bundle: Bundle,
    last_used: u64,
}

/// Per-chunk mapping into the shared format.
struct Remap {
    meta: Meta,
    scales: [u8; 256],
    sh0: [u8; 256],
    sh_n: [u8; 256],
}

pub struct LevelStream {
    pub settings: StreamSettings,
    /// None follows screen-space selection; Some requests a level subject to pool capacity.
    fixed_lod: Option<usize>,
    meta: LodMeta,
    /// Spatial index of regions and their previously selected IDs.
    spatial: SpatialOctree,
    shared: Meta,
    remaps: Vec<Option<Remap>>,
    /// Shared-quantized coordinate decoding, initialized on the first upload.
    centre_lookup: CentreLookup,
    /// Reused CPU staging storage for typical leaf uploads.
    upload_scratch: EncodedUpload,
    /// Codebook index of 0 in the shared SH codebook: the reserved centroid's texels.
    sh_zero: u8,
    chunks: BTreeMap<usize, Chunk>,
    in_flight: BTreeSet<usize>,
    leaves: Vec<LeafState>,
    /// Scratch IDs for the current spatial query, reused across frames.
    candidates: Vec<usize>,
    /// Reused each frame; sorted by visibility and priority, with leaf order breaking equal-priority ties.
    selection: Vec<SelectionCandidate>,
    /// Pending upload leaf IDs, reused while chunks arrive and fades complete.
    upload_order: Vec<usize>,
    free: Vec<(u32, u32)>,
    /// Palette slot → (chunk file, leaves using it).
    palettes: Vec<Option<(usize, u32)>>,
    distance_scale: f32,
    frame: u64,
    started: bool,
    /// Seconds, from `select`.
    now: f32,
    /// Replaced ranges dissolving out, and when they started to.
    fading_out: Vec<(Resident, f32)>,
    /// Scratch buffers retained across active crossfades.
    completed_fades: Vec<Resident>,
    fade_ranges: Vec<(u32, u32, f32)>,
    switches: u64,
    near_switches: u64,
    fades_cut: u64,
    hard_swaps: u64,
    /// Whether the last `apply` had to cut crossfades short for room: the selection is using more of the pool than
    /// leaves crossfades space, so `select` treats it like a shortage.
    squeezed: bool,
    /// The latest LOD changes, for debugging churn: (time, leaf, from, to, distance, in view).
    recent: std::collections::VecDeque<(f32, usize, usize, usize, f32, bool)>,
    /// Per leaf and LOD, the typical distance between neighbouring splats (level units): the square root of the
    /// leaf's dominant face area over its splat count.
    spacing: Vec<Vec<f32>>,
    /// Pixels per unit of size at unit distance (from the projection and view height), for the screen-space error.
    focal_px: f32,
    /// The nearest in-view leaf drawn at LOD 2 or coarser (level units), for the depth of field.
    sharp_m: f32,
    /// `lod_distance_m` scaled to what the pool holds: shrinks while leaves miss their target for lack of room and
    /// grows back while room is spare, slowly either way.
    detail_scale: f32,
    /// Optional diagnostic allocation trace. Disabled in ordinary runs to avoid retaining per-upload records.
    layout_diagnostics_enabled: bool,
    layout_enabled_from_frame: u64,
    layout_allocation_fingerprint: u64,
    layout_allocation_count: u64,
    layout_allocation_events_dropped: u64,
    layout_allocation_history: Vec<LayoutAllocationEvent>,
}

impl LevelStream {
    /// `chunk_metas[f]` is chunk file `f`'s `meta.json` (`None` if it could not be read: that file's ranges are
    /// skipped). The meta files are small; reading all of them up front gives the shared ranges and codebooks.
    pub fn new(
        meta: LodMeta,
        chunk_metas: &[Option<Vec<u8>>],
        mut settings: StreamSettings,
    ) -> Result<Self, String> {
        if settings.capacity == 0 {
            let lod0 = meta.counts.first().copied().unwrap_or(0) as f64;
            settings.capacity =
                ((lod0 * AUTO_POOL_SHARE) as u32).clamp(AUTO_POOL_MIN, AUTO_POOL_MAX);
        }
        let metas: Vec<Option<Meta>> = (0..meta.filenames.len())
            .map(|f| {
                chunk_metas
                    .get(f)
                    .and_then(|b| b.as_ref())
                    .and_then(|b| sog::parse_meta_json(b).ok())
            })
            .collect();
        let known: Vec<&Meta> = metas.iter().flatten().collect();
        if known.is_empty() {
            return Err("no chunk meta.json could be read".into());
        }
        let mut means_mins = [f32::MAX; 3];
        let mut means_maxs = [f32::MIN; 3];
        for m in &known {
            for i in 0..3 {
                means_mins[i] = means_mins[i].min(m.means_mins[i]);
                means_maxs[i] = means_maxs[i].max(m.means_maxs[i]);
            }
        }
        let scales = countersplat_assets::codebook::merge(
            known.iter().map(|m| m.scales_codebook.as_slice()),
        );
        let sh0 =
            countersplat_assets::codebook::merge(known.iter().map(|m| m.sh0_codebook.as_slice()));
        let mut sh_n =
            countersplat_assets::codebook::merge(known.iter().map(|m| m.sh_n_codebook.as_slice()));
        // The reserved centroid must decode to exactly zero, so one codebook entry is zero.
        let zero = (0..256)
            .min_by(|&a, &b| sh_n[a].abs().total_cmp(&sh_n[b].abs()))
            .unwrap_or(0);
        sh_n[zero] = 0.0;
        let shared = Meta {
            count: 0,
            means_mins,
            means_maxs,
            scales_codebook: scales,
            sh0_codebook: sh0,
            sh_n_codebook: sh_n,
            sh_n_count: 0,
        };
        let remaps = metas
            .into_iter()
            .map(|m| {
                m.map(|m| -> Result<Remap, String> {
                    Ok(Remap {
                        scales: countersplat_assets::codebook::remap(
                            &m.scales_codebook,
                            &shared.scales_codebook,
                        )?,
                        sh0: countersplat_assets::codebook::remap(
                            &m.sh0_codebook,
                            &shared.sh0_codebook,
                        )?,
                        sh_n: countersplat_assets::codebook::remap(
                            &m.sh_n_codebook,
                            &shared.sh_n_codebook,
                        )?,
                        meta: m,
                    })
                })
                .transpose()
            })
            .collect::<Result<Vec<_>, String>>()?;
        let coarsest = meta.levels().saturating_sub(1);
        let leaves = meta
            .leaves
            .iter()
            .map(|_| LeafState {
                want: Some(coarsest),
                have: None,
                distance: f32::MAX,
                visible: true,
                centered: false,
                coarsen_since: None,
                outside_since: None,
                offscreen_since: None,
            })
            .collect();
        let selection = Vec::with_capacity(meta.leaves.len());
        let spatial = SpatialOctree::new(&meta.leaves);
        // Sized for the largest centroid texture; `init_pool` fits it to the pool's.
        let palettes = vec![None; ((POOL_CENTROID_ROWS - 1) / PALETTE_ROWS) as usize];
        let spacing = meta
            .leaves
            .iter()
            .map(|leaf| {
                let mut size = [0, 1, 2].map(|i| (leaf.max[i] - leaf.min[i]).max(0.01));
                size.sort_by(f32::total_cmp);
                let area = size[2] * size[1];
                leaf.lods
                    .iter()
                    .map(|r| r.map_or(f32::MAX, |r| (area / r.count.max(1) as f32).sqrt()))
                    .collect()
            })
            .collect();
        Ok(Self {
            free: vec![(0, settings.capacity)],
            settings,
            meta,
            spatial,
            shared,
            remaps,
            centre_lookup: CentreLookup::Uninitialized,
            upload_scratch: EncodedUpload::default(),
            sh_zero: zero as u8,
            chunks: BTreeMap::new(),
            in_flight: BTreeSet::new(),
            leaves,
            candidates: Vec::new(),
            selection,
            upload_order: Vec::new(),
            palettes,
            distance_scale: 1.0,
            frame: 0,
            started: false,
            now: 0.0,
            fading_out: Vec::new(),
            completed_fades: Vec::new(),
            fade_ranges: Vec::new(),
            switches: 0,
            near_switches: 0,
            fades_cut: 0,
            hard_swaps: 0,
            squeezed: false,
            recent: Default::default(),
            detail_scale: 1.0,
            fixed_lod: None,
            spacing,
            focal_px: 1000.0,
            sharp_m: f32::MAX,
            layout_diagnostics_enabled: false,
            layout_enabled_from_frame: 0,
            layout_allocation_fingerprint: LAYOUT_HASH_OFFSET,
            layout_allocation_count: 0,
            layout_allocation_events_dropped: 0,
            layout_allocation_history: Vec::new(),
        })
    }

    /// Builds the stream from `lod-meta.json` and every chunk's `meta.json` given together as one JSON array (in file
    /// order; `null` for one that could not be read). The browser fetches them that way.
    pub fn from_json(
        lod_meta: &[u8],
        chunk_metas: &[u8],
        settings: StreamSettings,
    ) -> Result<Self, String> {
        let meta = LodMeta::parse(lod_meta)?;
        let all: serde_json::Value =
            serde_json::from_slice(chunk_metas).map_err(|e| format!("chunk metas: {e}"))?;
        let metas: Vec<Option<Vec<u8>>> = all
            .as_array()
            .ok_or("chunk metas: expected an array")?
            .iter()
            .map(|m| (!m.is_null()).then(|| serde_json::to_vec(m).unwrap_or_default()))
            .collect();
        Self::new(meta, &metas, settings)
    }

    /// The shared position range and codebooks, for `scene::pool`.
    pub fn shared_meta(&self) -> &Meta {
        &self.shared
    }

    /// Every SH-bearing file can occupy at most one palette slot. Preserve all
    /// slots the old device-sized texture could provide, up to the number of
    /// files, and omit partial trailing slots that could never be used.
    pub fn palette_texture_rows(&self, max_texture_side: u32) -> u32 {
        let slots = POOL_CENTROID_ROWS.min(max_texture_side).saturating_sub(1) / PALETTE_ROWS;
        let files = self
            .remaps
            .iter()
            .flatten()
            .filter(|r| r.meta.sh_n_count > 0)
            .take(slots as usize)
            .count() as u32;
        1 + files * PALETTE_ROWS
    }

    /// The level's tree.
    pub fn lod_meta(&self) -> &LodMeta {
        &self.meta
    }

    /// Picks each leaf's LOD for a camera at `eye` looking along `forward`, both in the level's splat space, at time
    /// `now_s` (seconds). `clip` (projection · view · level model, column-major) gives the view frustum; without it
    /// only leaves entirely behind the camera count as out of view.
    ///
    /// Every leaf in the draw distance starts at its coarsest LOD; then, leaves in view first and nearest first, each
    /// is refined to the finest LOD its distance allows (with hysteresis, the out-of-view bias and the keep delay)
    /// whose extra splats still fit the pool. Detail is spent where it is seen, and the selection always fits.
    pub fn select(
        &mut self,
        eye: [f32; 3],
        forward: [f32; 3],
        clip: Option<&[f32; 16]>,
        now_s: f32,
    ) {
        self.frame += 1;
        self.now = now_s;
        let budget = self.prepare_selection(eye, forward, clip, now_s);
        let (budget, refined) = self.refine_selection(budget);
        self.sharp_m = self
            .leaves
            .iter()
            .filter(|s| s.visible && s.want.is_some_and(|w| w >= 2))
            .map(|s| s.distance)
            .fold(f32::MAX, f32::min);
        // How much of the configured detail the pool allowed: the share of leaves that got their distance's LOD.
        let selected = self.selection.len();
        self.distance_scale = if selected == 0 {
            1.0
        } else {
            refined as f32 / selected as f32
        };
        // When the pool cannot hold every leaf's target, leaves at the edge of the budget trade detail back and forth
        // as the camera moves. Shrinking the LOD distance until the targets fit makes them stable again (the
        // hysteresis and keep delay then hold); spare room lets it grow back. Slowly, so it does not oscillate.
        // Between "a few leaves short" and "15 % of the pool spare" it holds still, so it settles instead of hunting.
        // Crossfades cut short for room count as a shortage too: the selection must leave them space.
        let short = (selected - refined) * 50 > selected || std::mem::take(&mut self.squeezed);
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

    /// Gather each eligible leaf's target and pay for its coarsest LOD. This computes distance and priority once.
    fn prepare_selection(
        &mut self,
        eye: [f32; 3],
        forward: [f32; 3],
        clip: Option<&[f32; 16]>,
        now_s: f32,
    ) -> i64 {
        let planes = clip.map(|c| {
            (
                frustum_planes(c, ENTER_MARGIN),
                frustum_planes(c, LEAVE_MARGIN),
            )
        });
        let center_planes = (self.settings.center_lod0_m > 0.0)
            .then(|| clip.map(|matrix| frustum_planes(matrix, 0.5)))
            .flatten();
        let ellipses = match self.settings.draw_region {
            DrawRegion::ForwardEllipse {
                rear_fraction,
                side_fraction,
            } => Some((
                ForwardEllipseQuery::new(
                    eye,
                    forward,
                    self.settings.max_distance_m,
                    rear_fraction,
                    side_fraction,
                    1.0,
                ),
                ForwardEllipseQuery::new(
                    eye,
                    forward,
                    self.settings.max_distance_m,
                    rear_fraction,
                    side_fraction,
                    1.20,
                ),
            )),
            DrawRegion::Radial => None,
        };
        let mut budget =
            i64::from(self.settings.capacity) * i64::from(self.settings.select_share_percent) / 100;
        self.selection.clear();
        // The octree indexes the level regions; active IDs are also revisited so leaving regions can fade out.
        self.candidates.clear();
        match &ellipses {
            Some((_, retention)) => self
                .spatial
                .forward_candidates(retention, &mut self.candidates),
            None => self.candidates.extend(0..self.leaves.len()),
        };
        for &i in &self.candidates {
            let state = &mut self.leaves[i];
            let leaf = &self.meta.leaves[i];
            let distance = effective_distance(eye, leaf.min, leaf.max);
            state.distance = distance;
            let in_region = match &ellipses {
                None => distance <= self.settings.max_distance_m,
                Some((enter, retention)) => {
                    // A larger leave footprint keeps small movements and turns from churning the edge.
                    let query = if state.want.is_some() && self.started {
                        retention
                    } else {
                        enter
                    };
                    query.intersects(leaf.min, leaf.max)
                }
            };
            if in_region {
                state.outside_since = None;
            } else if matches!(self.settings.draw_region, DrawRegion::ForwardEllipse { .. })
                && state.have.is_some()
            {
                // Keep resident chunks briefly beyond the outer edge, then let their normal fade retire them.
                let since = *state.outside_since.get_or_insert(now_s);
                if now_s - since >= 1.25 {
                    state.want = None;
                    continue;
                }
            } else {
                state.outside_since = None;
                state.want = None;
                continue;
            }
            let Some(coarse) = coarsest_available(&leaf.lods) else {
                state.want = None;
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
            state.centered = state.visible
                && distance <= self.settings.center_lod0_m
                && center_planes
                    .as_ref()
                    .is_some_and(|planes| box_in_frustum(planes, leaf.min, leaf.max));
            if !state.visible && distance > self.settings.offscreen_keep_m {
                if state.have.is_some() {
                    let since = *state.offscreen_since.get_or_insert(now_s);
                    if now_s - since >= 1.25 {
                        state.want = None;
                        continue;
                    }
                } else {
                    state.offscreen_since = None;
                    state.want = None;
                    continue;
                }
            } else {
                state.offscreen_since = None;
            }
            let target = if let Some(fixed) = self.fixed_lod {
                // A manual request takes effect immediately; the allocator may still choose
                // an available coarser LOD when this leaf or the GPU budget requires it.
                state.coarsen_since = None;
                fixed.min(coarse)
            } else if state.centered
                || (state.visible
                    && self.settings.near_lod0_m > 0.0
                    && distance <= self.settings.near_lod0_m)
            {
                // Nearby detail has priority even when the adaptive budget relaxes distant detail.
                state.coarsen_since = None;
                0
            } else {
                // Apply the out-of-view bias before hysteresis, so both compare the same level.
                let bias = if state.visible {
                    0.0
                } else {
                    self.settings.behind_bias as f32
                };
                let x = level_at_values(
                    &self.spacing,
                    &self.settings,
                    self.detail_scale,
                    self.focal_px,
                    i,
                    distance,
                ) + bias;
                let mut lod = x.floor() as usize;
                if let Some(have) = state.have {
                    if x > have.lod as f32 - 0.2 && x < have.lod as f32 + 1.2 {
                        lod = have.lod;
                    }
                }
                match state.have {
                    Some(have) if lod > have.lod => {
                        let since = *state.coarsen_since.get_or_insert(now_s);
                        if now_s - since < self.settings.keep_s {
                            lod = have.lod;
                        }
                    }
                    _ => state.coarsen_since = None,
                }
                lod.min(coarse)
            };
            let settled = state.have.is_some_and(|h| h.lod <= target);
            self.selection.push(SelectionCandidate {
                leaf: i,
                target,
                coarse,
                priority: distance * if settled { STICKY } else { 1.0 },
                visible: state.visible,
                centered: state.centered,
            });
            budget -= leaf_count(leaf, coarse);
        }
        self.spatial
            .set_active(self.selection.iter().map(|candidate| candidate.leaf));
        budget
    }

    /// Spend the remaining pool on in-view leaves first, then by sticky distance, preserving leaf order on ties.
    fn refine_selection(&mut self, mut budget: i64) -> (i64, usize) {
        // The original stable sort kept equal priorities in leaf order. Including the unique index yields the same
        // total order without allocating a temporary sort buffer.
        self.selection.sort_unstable_by(|a, b| {
            b.centered
                .cmp(&a.centered)
                .then(b.visible.cmp(&a.visible))
                .then(a.priority.total_cmp(&b.priority))
                .then(a.leaf.cmp(&b.leaf))
        });
        let mut refined = 0;
        for candidate in &self.selection {
            let leaf = &self.meta.leaves[candidate.leaf];
            let base = leaf_count(leaf, candidate.coarse);
            let chosen = (candidate.target..=candidate.coarse)
                .filter(|&l| leaf.lods[l].is_some())
                .find(|&l| leaf_count(leaf, l) - base <= budget)
                .unwrap_or(candidate.coarse);
            budget -= leaf_count(leaf, chosen) - base;
            self.leaves[candidate.leaf].want = Some(chosen);
            refined += usize::from(chosen == candidate.target);
        }
        (budget, refined)
    }

    /// Chunk files to fetch now, as (file index, chunk folder), marked as in flight. Nearby visible leaves request
    /// their target directly; other empty leaves ask for their coarsest file first. In-view, nearby work has priority.
    pub fn requests(&mut self) -> Vec<(usize, String)> {
        self.requests_up_to(usize::MAX)
    }

    /// Request no more than the host can start immediately. Avoids queuing work
    /// behind busy decoders, so priorities can follow the latest camera position.
    pub fn requests_up_to(&mut self, available: usize) -> Vec<(usize, String)> {
        let room = self
            .settings
            .max_in_flight
            .saturating_sub(self.in_flight.len())
            .min(available);
        if room == 0 || !self.started {
            return Vec::new();
        }
        let mut wanted: Vec<(bool, bool, bool, f32, usize)> = Vec::new();
        for (state, leaf) in self.leaves.iter().zip(&self.meta.leaves) {
            let Some(want) = state.want else { continue };
            if state.have.is_some_and(|h| h.lod == want) {
                continue;
            }
            let prioritize_near = state.centered
                || (state.visible
                    && self.settings.near_lod0_m > 0.0
                    && state.distance <= self.settings.near_lod0_m);
            let lod = if state.have.is_none() && !prioritize_near {
                coarsest_available(&leaf.lods)
            } else {
                Some(want)
            };
            let Some(range) = lod.and_then(|l| leaf.lods[l]) else {
                continue;
            };
            if self.chunks.contains_key(&range.file)
                || self.in_flight.contains(&range.file)
                || self.remaps[range.file].is_none()
            {
                continue;
            }
            wanted.push((
                !state.centered,
                state.have.is_some(),
                !state.visible,
                state.distance,
                range.file,
            ));
        }
        wanted.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.total_cmp(&b.3))
        });
        // Then prefetch: files the focus is about to need, so detail is ready when it arrives.
        let ahead = self
            .prefetch()
            .into_iter()
            .map(|(distance, file)| (true, true, true, distance, file));
        let mut out = Vec::new();
        for (_, _, _, _, file) in wanted.into_iter().chain(ahead) {
            if out.len() >= room {
                break;
            }
            if self.chunks.contains_key(&file) || self.remaps[file].is_none() {
                continue;
            }
            if self.in_flight.insert(file) {
                out.push((file, self.meta.dir_of(file).unwrap_or_default().to_string()));
            }
        }
        out
    }

    /// Chunk files worth having before they are wanted, nearest first, as (distance, file): for each leaf in view
    /// that would want its next finer LOD if it were `PREFETCH_REACH` times nearer (far enough ahead to arrive in
    /// time at 100 km/h), that LOD's file. Fetched
    /// when nothing more urgent is, and kept in memory while they qualify, so a leaf refines as soon as the focus is
    /// near enough instead of waiting for its chunk to download and decode.
    fn prefetch(&self) -> Vec<(f32, usize)> {
        if self.fixed_lod.is_some() {
            return Vec::new();
        }
        const PREFETCH_REACH: f32 = 2.5;
        let mut files: BTreeMap<usize, f32> = BTreeMap::new();
        for (i, (state, leaf)) in self.leaves.iter().zip(&self.meta.leaves).enumerate() {
            let Some(want) = state.want.filter(|&w| w > 0) else {
                continue;
            };
            if !state.visible {
                continue;
            }
            // Would it want a finer LOD `PREFETCH_REACH` times nearer?
            if self.level_at(i, state.distance / PREFETCH_REACH).floor() as usize >= want {
                continue;
            }
            if let Some(range) = leaf.lods[want - 1] {
                let d = files.entry(range.file).or_insert(f32::MAX);
                *d = d.min(state.distance);
            }
        }
        let mut out: Vec<(f32, usize)> = files.into_iter().map(|(f, d)| (d, f)).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// Whether fetched metadata matches the metadata used to construct this stream's remapping.
    pub fn accepts_chunk_meta(&self, file: usize, meta: &Meta) -> bool {
        self.remaps
            .get(file)
            .and_then(Option::as_ref)
            .is_some_and(|r| &r.meta == meta)
    }

    /// A requested chunk file arrived.
    pub fn insert(&mut self, file: usize, bundle: Bundle) {
        self.in_flight.remove(&file);
        self.chunks.insert(
            file,
            Chunk {
                bundle,
                last_used: self.frame,
            },
        );
    }

    /// A requested chunk file could not be loaded; it may be requested again later.
    pub fn failed(&mut self, file: usize) {
        self.in_flight.remove(&file);
    }

    /// Writes the reserved zero centroid and sizes the palettes to the pool's centroid texture. Call once on a new pool.
    pub fn init_pool(&mut self, pool: &GsplatScene, queue: &wgpu::Queue) {
        self.palettes =
            vec![None; (pool.centroid_rows().saturating_sub(1) / PALETTE_ROWS) as usize];
        let row = zero_sh_centroid_row(self.sh_zero);
        pool.write_centroid_rows(queue, 0, &row);
        // Only this pool has the reserved row. Static scenes start with the
        // flag at zero, so their valid palette label 0 still evaluates SH.
        pool.set_uniform(queue, "sh_zero_reserved", &1u32.to_le_bytes());
    }

    /// Uploads leaves whose wanted range is loaded, in view and nearest first, within the per-frame budget; releases
    /// ranges that have faded out; hands the pool the ranges still fading; then drops chunks nothing pending needs.
    /// Returns the splats uploaded.
    pub fn apply(&mut self, pool: &GsplatScene, queue: &wgpu::Queue) -> u32 {
        let now = self.now;
        // Ranges done fading out give their slots back.
        let fade_s = self.settings.fade_s;
        let mut done = std::mem::take(&mut self.completed_fades);
        collect_completed_fades(&mut self.fading_out, &mut done, now, fade_s);
        for old in done.drain(..) {
            self.free_resident(pool, queue, old);
        }
        self.completed_fades = done;
        // Leaves past the draw distance fade out.
        for i in 0..self.leaves.len() {
            if self.leaves[i].want.is_none() {
                if let Some(old) = self.leaves[i].have.take() {
                    self.retire(pool, queue, old);
                }
            }
        }
        let mut order = std::mem::take(&mut self.upload_order);
        self.fill_upload_order(&mut order);
        let mut budget = self.settings.uploads_per_frame;
        let mut uploaded = 0;
        for &i in &order {
            let Some(want) = self.leaves[i].want else {
                continue;
            };
            let Some((lod, range)) = loaded_range(
                &self.meta.leaves[i].lods,
                want,
                self.leaves[i].have.is_some(),
                |file| self.chunks.contains_key(&file),
            ) else {
                continue;
            };
            // Do not replace a range halfway through its fade-in. Restarting its
            // fade-out at full opacity produces a visible flash under rapid LOD changes.
            if self.leaves[i]
                .have
                .is_some_and(|resident| !resident_is_settled(resident, now, fade_s))
            {
                continue;
            }
            if range.count > budget && uploaded > 0 {
                break;
            }
            // A new range normally goes in beside the old one, which fades out, so the leaf never blinks out. Out of
            // room, optional fade cuts finish oldest crossfades one at a time. If compaction
            // cannot make room, visible leaves keep their old LOD in smooth mode.
            let mut start = self.allocate(range.count);
            // With smooth transitions enabled, wait for existing fades to finish rather than
            // cutting hundreds of fading ranges short when the player moves or changes distance.
            if start.is_none() && !self.settings.allow_fade_cuts && !self.fading_out.is_empty() {
                continue;
            }
            while start.is_none() && !self.fading_out.is_empty() {
                let oldest = (0..self.fading_out.len())
                    .min_by(|&a, &b| self.fading_out[a].1.total_cmp(&self.fading_out[b].1))
                    .expect("not empty");
                let (old, _) = self.fading_out.swap_remove(oldest);
                self.free_resident(pool, queue, old);
                self.fades_cut += 1;
                self.squeezed = true;
                start = self.allocate(range.count);
            }
            // When space is fragmented, move one resident range into a hole and retry on
            // the next frame. This keeps existing chunks visible while creating a contiguous
            // range for the higher-priority upload.
            if start.is_none() && self.compact_one(pool, queue, range.count) {
                break;
            }
            let mut hard_swap = false;
            let start = match start {
                Some(start) => start,
                None => {
                    let Some(old) = self.leaves[i].have else {
                        continue;
                    };
                    // Never abruptly replace a visible range in smooth mode. Other
                    // leaves can coarsen and finish fading to make room on a later frame.
                    if fade_s > 0.0 && !self.settings.allow_fade_cuts && self.leaves[i].visible {
                        continue;
                    }
                    self.release(old.start, old.count);
                    match self.allocate(range.count) {
                        Some(start) => {
                            self.hard_swaps += 1;
                            hard_swap = true;
                            pool.release(queue, old.start, old.count);
                            if let Some(p) = old.palette {
                                self.unuse_palette(p);
                            }
                            self.leaves[i].have = None;
                            start
                        }
                        None => {
                            self.take_exact(old.start, old.count);
                            continue;
                        }
                    }
                }
            };
            let palette = if lod < self.settings.sh_lods {
                self.palette_for(range.file, pool, queue)
            } else {
                None
            };
            self.upload(pool, queue, range, start, palette);
            pool.write_lod(queue, start, range.count, lod, i);
            if let Some(old) = self.leaves[i].have {
                let state = &self.leaves[i];
                self.recent
                    .push_back((now, i, old.lod, lod, state.distance, state.visible));
                if self.recent.len() > 512 {
                    self.recent.pop_front();
                }
                self.switches += 1;
                if self.leaves[i].visible && self.leaves[i].distance <= self.settings.lod_distance_m
                {
                    self.near_switches += 1;
                }
            }
            if let Some(old) = self.leaves[i].have.take() {
                self.retire(pool, queue, old);
            }
            self.leaves[i].have = Some(Resident {
                lod,
                start,
                count: range.count,
                palette,
                // A hard swap has no old range to fade over: draw the replacement immediately.
                born: if hard_swap { now - fade_s } else { now },
            });
            self.record_layout_allocation(i, lod, range, start, palette);
            if let Some(chunk) = self.chunks.get_mut(&range.file) {
                chunk.last_used = self.frame;
            }
            budget = budget.saturating_sub(range.count);
            uploaded += range.count;
        }
        self.upload_order = order;
        pool.set_active(queue, self.high_water());
        let mut fades = std::mem::take(&mut self.fade_ranges);
        self.fill_fades(&mut fades);
        pool.set_fades(queue, &fades);
        self.fade_ranges = fades;
        self.evict();
        uploaded
    }

    // Preserve the stable priority order while reusing the backing allocation.
    fn fill_upload_order(&self, order: &mut Vec<usize>) {
        order.clear();
        order.extend((0..self.leaves.len()).filter(|&i| {
            self.leaves[i]
                .want
                .is_some_and(|w| self.leaves[i].have.is_none_or(|h| h.lod != w))
                || self.wants_palette(i)
        }));
        order.sort_by(|&a, &b| {
            let (sa, sb) = (&self.leaves[a], &self.leaves[b]);
            sb.centered
                .cmp(&sa.centered)
                .then(sa.have.is_some().cmp(&sb.have.is_some()))
                .then(sb.visible.cmp(&sa.visible))
                .then(sa.distance.total_cmp(&sb.distance))
        });
    }

    /// Whether a resident leaf showing a LOD that gets spherical harmonics went without them (every palette slot was
    /// taken when it was uploaded) and could have them now: its chunk is loaded and a slot is free or already holds
    /// its chunk. Such a leaf is uploaded again, crossfading to the same LOD with its view-dependent colour.
    fn wants_palette(&self, i: usize) -> bool {
        let (state, leaf) = (&self.leaves[i], &self.meta.leaves[i]);
        let Some(have) = state.have else { return false };
        if have.palette.is_some()
            || have.lod >= self.settings.sh_lods
            || state.want != Some(have.lod)
        {
            return false;
        }
        let Some(range) = leaf.lods[have.lod] else {
            return false;
        };
        let has_sh = self.remaps[range.file]
            .as_ref()
            .is_some_and(|m| m.meta.sh_n_count > 0);
        let room = self
            .palettes
            .iter()
            .any(|p| p.is_none_or(|(f, users)| users == 0 || f == range.file));
        has_sh && room && self.chunks.contains_key(&range.file)
    }

    /// The ranges crossfading now, by start: (first slot, one past the last, opacity share).
    /// Old and new ranges crossfade together to avoid the bright flash of drawing both at full opacity.
    pub fn fades(&self) -> Vec<(u32, u32, f32)> {
        let mut out = Vec::new();
        self.fill_fades(&mut out);
        out
    }

    fn fill_fades(&self, out: &mut Vec<(u32, u32, f32)>) {
        out.clear();
        let fade_s = self.settings.fade_s;
        if fade_s <= 0.0 {
            return;
        }
        let progress = |t: f32| (t / fade_s).clamp(0.0, 1.0);
        for state in &self.leaves {
            if let Some(r) = state.have {
                if self.now - r.born < fade_s {
                    out.push((r.start, r.start + r.count, progress(self.now - r.born)));
                }
            }
        }
        for (r, t0) in &self.fading_out {
            out.push((r.start, r.start + r.count, 1.0 - progress(self.now - t0)));
        }
        out.sort_by_key(|f| f.0);
    }

    fn fade_count(&self) -> usize {
        if self.settings.fade_s <= 0.0 {
            return 0;
        }
        self.leaves
            .iter()
            .filter_map(|state| state.have)
            .filter(|resident| self.now - resident.born < self.settings.fade_s)
            .count()
            + self.fading_out.len()
    }

    /// A range a leaf no longer shows: it fades out, or goes at once when fading is off.
    fn retire(&mut self, pool: &GsplatScene, queue: &wgpu::Queue, old: Resident) {
        if self.settings.fade_s > 0.0 {
            self.fading_out
                .push((old, retirement_start(old, self.now, self.settings.fade_s)));
        } else {
            self.free_resident(pool, queue, old);
        }
    }

    fn free_resident(&mut self, pool: &GsplatScene, queue: &wgpu::Queue, old: Resident) {
        pool.release(queue, old.start, old.count);
        self.release(old.start, old.count);
        if let Some(p) = old.palette {
            self.unuse_palette(p);
        }
    }

    /// How far from the LOD centre the level is sharp: the nearest leaf in view drawn at LOD 2 or coarser (by the
    /// distance bands, half way through the LOD 1 band). The depth of field starts softening there.
    pub fn sharp_distance(&self) -> f32 {
        if self.settings.lod_error_px > 0.0 {
            self.sharp_m.min(self.settings.max_distance_m)
        } else {
            self.settings.lod_distance_m * self.detail_scale * 1.5
        }
    }

    /// The view's pixels per unit of size at unit distance: projection[5] (1 / tan of half the vertical field of view)
    /// times half the view height. The screen-space error is measured at a 1080-pixel-tall view whatever the window,
    /// so the detail does not change with the window size; pass `projection[5] * 540`.
    pub fn set_focal_px(&mut self, focal_px: f32) {
        self.focal_px = focal_px.max(1.0);
    }

    /// Request one LOD for every leaf, or restore automatic selection with `None`.
    /// Missing leaf LODs use the nearest available level at or above the request, or the
    /// coarsest available level if none is coarser. Pool limits may also reduce detail.
    pub fn set_fixed_lod(&mut self, lod: Option<usize>) -> Result<(), &'static str> {
        if lod.is_some_and(|value| value >= self.meta.levels()) {
            return Err("requested LOD is not available in this level");
        }
        self.fixed_lod = lod;
        Ok(())
    }

    /// The continuous level a leaf wants at `distance`: by screen-space error, the LOD whose splats land
    /// `lod_error_px` apart, interpolated between levels (2.3 is 30 % of the way from LOD 2 to LOD 3); by distance
    /// bands, 0 inside d0, 1 at 2·d0, 2 at 4·d0.
    fn level_at(&self, leaf: usize, distance: f32) -> f32 {
        level_at_values(
            &self.spacing,
            &self.settings,
            self.detail_scale,
            self.focal_px,
            leaf,
            distance,
        )
    }

    /// Checks the pool bookkeeping: resident and fading ranges never overlap each other or the free list, and
    /// together with it they cover the pool exactly. Returns the first problem found, for tests and probes.
    pub fn check_pool(&self) -> Result<(), String> {
        let mut used: Vec<(u32, u32, String)> = self
            .leaves
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.have
                    .map(|h| (h.start, h.count, format!("leaf {i} LOD {}", h.lod)))
            })
            .chain(
                self.fading_out
                    .iter()
                    .map(|(r, _)| (r.start, r.count, format!("fading LOD {}", r.lod))),
            )
            .chain(self.free.iter().map(|&(s, c)| (s, c, "free".to_string())))
            .collect();
        used.sort_by_key(|u| u.0);
        let mut at = 0u32;
        for (start, count, what) in &used {
            if *start < at {
                return Err(format!(
                    "{what} at {start}..{} overlaps what ends at {at}",
                    start + count
                ));
            }
            if *start > at {
                return Err(format!("slots {at}..{start} are neither used nor free"));
            }
            at = start + count;
        }
        if at != self.settings.capacity {
            return Err(format!(
                "bookkeeping ends at {at}, not the capacity {}",
                self.settings.capacity
            ));
        }
        Ok(())
    }

    /// Every leaf with something resident, for the debug layer (`Frame::set_lod_debug`).
    pub fn debug_leaves(&self) -> Vec<DebugLeaf> {
        self.leaves
            .iter()
            .zip(&self.meta.leaves)
            .filter_map(|(state, leaf)| {
                let have = state.have?;
                Some(DebugLeaf {
                    min: leaf.min,
                    max: leaf.max,
                    lod: have.lod,
                    pending: state.want.is_some_and(|w| w != have.lod),
                    visible: state.visible,
                })
            })
            .collect()
    }

    /// The latest LOD changes as a JSON array of [time, leaf, from, to, distance, in view], oldest first.
    pub fn recent_switches_json(&self) -> String {
        let rows: Vec<String> = self
            .recent
            .iter()
            .map(|(t, i, a, b, d, v)| format!("[{t:.3},{i},{a},{b},{d:.1},{}]", u8::from(*v)))
            .collect();
        format!("[{}]", rows.join(","))
    }

    /// The `n` leaves nearest the LOD centre, nearest first, as JSON objects: distance (effective), box size, wanted
    /// and resident LOD, in view, and whether the wanted range's chunk is loaded or in flight. For debugging pop-in.
    pub fn nearest_leaves_json(&self, n: usize) -> String {
        let mut order: Vec<usize> = (0..self.leaves.len()).collect();
        order.sort_by(|&a, &b| self.leaves[a].distance.total_cmp(&self.leaves[b].distance));
        let rows: Vec<String> = order
            .into_iter()
            .take(n)
            .map(|i| {
                let (state, leaf) = (&self.leaves[i], &self.meta.leaves[i]);
                let file = state.want.and_then(|w| leaf.lods[w]).map(|r| r.file);
                let size = [0, 1, 2].map(|k| (leaf.max[k] - leaf.min[k]).round());
                let counts: Vec<u32> = leaf.lods.iter().map(|r| r.map_or(0, |r| r.count)).collect();
                format!(
                    "{{\"leaf\":{i},\"d\":{:.1},\"size\":{size:?},\"want\":{:?},\"have\":{:?},\"visible\":{},\"loaded\":{},\"in_flight\":{},\"sh\":{},\"counts\":{counts:?}}}",
                    state.distance,
                    state.want,
                    state.have.map(|h| h.lod),
                    state.visible,
                    file.is_some_and(|f| self.chunks.contains_key(&f)),
                    file.is_some_and(|f| self.in_flight.contains(&f)),
                    state.have.is_some_and(|h| h.palette.is_some()),
                )
            })
            .collect();
        format!("[{}]", rows.join(","))
    }

    /// A small sample of unresolved uploads, for diagnosing stalled or missing detail.
    pub fn pending_details(&self, limit: usize) -> Vec<serde_json::Value> {
        self.leaves
            .iter()
            .zip(&self.meta.leaves)
            .enumerate()
            .filter_map(|(i, (state, leaf))| {
                let want = state.want?;
                if state.have.is_some_and(|have| have.lod == want) {
                    return None;
                }
                let range = leaf.lods[want]?;
                Some(serde_json::json!({
                    "leaf": i, "distance": state.distance, "centered": state.centered,
                    "wanted_lod": want, "resident_lod": state.have.map(|h| h.lod),
                    "wanted_count": range.count, "resident_count": state.have.map(|h| h.count),
                    "loaded": self.chunks.contains_key(&range.file),
                    "in_flight": self.in_flight.contains(&range.file),
                }))
            })
            .take(limit)
            .collect()
    }

    pub fn stats(&self) -> StreamStats {
        let mut per_lod = vec![0u32; self.meta.levels()];
        let mut empty = 0;
        let mut pending = 0;
        let mut resident = 0;
        let mut hidden = 0;
        let visible = self
            .leaves
            .iter()
            .filter(|s| s.want.is_some() && s.visible)
            .count() as u32;
        for s in &self.leaves {
            match (s.have, s.want) {
                (_, None) => hidden += 1,
                (Some(h), Some(want)) => {
                    per_lod[h.lod] += 1;
                    resident += h.count;
                    if h.lod != want {
                        pending += 1;
                    }
                }
                (None, Some(_)) => empty += 1,
            }
        }
        let wanted = self
            .leaves
            .iter()
            .zip(&self.meta.leaves)
            .filter_map(|(s, l)| s.want.and_then(|w| l.lods[w]).map(|r| u64::from(r.count)))
            .sum();
        StreamStats {
            resident_splats: resident,
            wanted_splats: wanted,
            active_slots: self.high_water(),
            free_slots: self.free.iter().map(|(_, len)| len).sum(),
            largest_free_range: self.free.iter().map(|(_, len)| *len).max().unwrap_or(0),
            leaves_per_lod: per_lod,
            octree_nodes: self.spatial.node_count(),
            leaves_empty: empty,
            leaves_hidden: hidden,
            leaves_pending: pending,
            chunks_loaded: self.chunks.len(),
            chunks_in_flight: self.in_flight.len(),
            leaves_visible: visible,
            fading: self.fade_count() as u32,
            switches: self.switches,
            near_switches: self.near_switches,
            palettes_in_use: self
                .palettes
                .iter()
                .filter(|p| p.is_some_and(|(_, users)| users > 0))
                .count() as u32,
            missing_sh: self
                .leaves
                .iter()
                .zip(&self.meta.leaves)
                .filter(|(s, l)| {
                    s.have.is_some_and(|h| {
                        h.lod < self.settings.sh_lods
                            && h.palette.is_none()
                            && l.lods[h.lod].is_some_and(|r| {
                                self.remaps[r.file]
                                    .as_ref()
                                    .is_some_and(|m| m.meta.sh_n_count > 0)
                            })
                    })
                })
                .count() as u32,
            fades_cut: self.fades_cut,
            hard_swaps: self.hard_swaps,
            detail_met: self.distance_scale,
            detail_scale: self.detail_scale,
        }
    }

    /// Enables or clears diagnostic fingerprints/traces without changing selection or allocation behavior.
    /// Call before streaming arrivals to capture the full allocation history.
    pub fn set_layout_diagnostics(&mut self, enabled: bool) {
        self.layout_diagnostics_enabled = enabled;
        self.layout_enabled_from_frame = self.frame;
        self.layout_allocation_fingerprint = LAYOUT_HASH_OFFSET;
        self.layout_allocation_count = 0;
        self.layout_allocation_events_dropped = 0;
        self.layout_allocation_history.clear();
        if enabled {
            self.layout_allocation_history
                .reserve(MAX_LAYOUT_ALLOCATION_EVENTS.min(self.leaves.len() * 4));
        }
    }

    /// Returns a snapshot only when diagnostics were explicitly enabled. The resident mapping covers each leaf's
    /// current range and excludes ranges still fading out.
    pub fn layout_diagnostics(&self) -> Option<StreamLayoutDiagnostics> {
        self.layout_diagnostics_enabled
            .then(|| self.layout_diagnostics_snapshot())
    }

    fn layout_diagnostics_snapshot(&self) -> StreamLayoutDiagnostics {
        let resident_ranges: Vec<_> = self
            .leaves
            .iter()
            .enumerate()
            .filter_map(|(leaf, state)| {
                let have = state.have?;
                let range = self.meta.leaves.get(leaf)?.lods.get(have.lod)?.as_ref()?;
                Some(ResidentRangeLayout {
                    leaf,
                    file: range.file,
                    lod: have.lod,
                    source_offset: range.offset,
                    count: have.count,
                    pool_start: have.start,
                    palette: have.palette,
                })
            })
            .collect();
        let resident_fingerprint = format!("{:016x}", hash_resident_layout(&resident_ranges));
        StreamLayoutDiagnostics {
            enabled_from_frame: self.layout_enabled_from_frame,
            resident_fingerprint,
            allocation_fingerprint: format!("{:016x}", self.layout_allocation_fingerprint),
            allocation_count: self.layout_allocation_count,
            allocation_events_dropped: self.layout_allocation_events_dropped,
            resident_ranges,
            allocation_history: self.layout_allocation_history.clone(),
        }
    }

    fn record_layout_allocation(
        &mut self,
        leaf: usize,
        lod: usize,
        range: Range,
        start: u32,
        palette: Option<usize>,
    ) {
        if !self.layout_diagnostics_enabled {
            return;
        }
        let event = LayoutAllocationEvent {
            frame: self.frame,
            time_s: self.now,
            leaf,
            file: range.file,
            lod,
            source_offset: range.offset,
            count: range.count,
            pool_start: start,
            palette,
        };
        hash_allocation_event(&mut self.layout_allocation_fingerprint, &event);
        self.layout_allocation_count += 1;
        if self.layout_allocation_history.len() < MAX_LAYOUT_ALLOCATION_EVENTS {
            self.layout_allocation_history.push(event);
        } else {
            self.layout_allocation_events_dropped += 1;
        }
    }

    /// Re-encodes a leaf range from its chunk into the shared format and writes it at `start`.
    fn upload(
        &mut self,
        pool: &GsplatScene,
        queue: &wgpu::Queue,
        range: Range,
        start: u32,
        palette: Option<usize>,
    ) {
        let (Some(chunk), Some(remap)) = (
            self.chunks.get(&range.file),
            self.remaps[range.file].as_ref(),
        ) else {
            return;
        };
        let lookup = self.centre_lookup.for_upload(&self.shared);
        let mut temporary = EncodedUpload::default();
        let encoded = if range.count as usize <= UPLOAD_SCRATCH_SPLATS {
            &mut self.upload_scratch
        } else {
            &mut temporary
        };
        encode_upload_into(
            encoded,
            &chunk.bundle,
            remap,
            &self.shared,
            range,
            palette,
            lookup,
        );
        let first = range.offset as usize * 4;
        let last = (range.offset as usize + range.count as usize) * 4;
        let quats = &chunk.bundle.textures["quats"].rgba
            [first..last.min(chunk.bundle.textures["quats"].rgba.len())];
        pool.write_splats(
            queue,
            start,
            &[
                ("quats", quats),
                ("scales", &encoded.scales),
                ("sh0", &encoded.sh0),
                ("sh_labels", &encoded.labels),
            ],
            &encoded.centres,
        );
    }

    /// A palette slot holding `file`'s SH centroids, uploading them if needed. `None` when every slot is in use.
    fn palette_for(
        &mut self,
        file: usize,
        pool: &GsplatScene,
        queue: &wgpu::Queue,
    ) -> Option<usize> {
        if let Some(slot) = self
            .palettes
            .iter()
            .position(|p| p.is_some_and(|(f, _)| f == file))
        {
            if let Some((_, users)) = self.palettes[slot].as_mut() {
                *users += 1;
            }
            return Some(slot);
        }
        let (chunk, remap) = (self.chunks.get(&file)?, self.remaps[file].as_ref()?);
        if remap.meta.sh_n_count == 0 {
            // No spherical harmonics in this chunk: its labels point at the reserved zero centroid.
            return None;
        }
        let slot = self
            .palettes
            .iter()
            .position(|p| p.is_none_or(|(_, users)| users == 0))?;
        let centroids = &chunk.bundle.textures["sh_centroids"];
        let data = encode_sh_palette(centroids, &remap.sh_n, self.sh_zero)?;
        pool.write_centroid_rows(queue, 1 + slot as u32 * PALETTE_ROWS, &data);
        self.palettes[slot] = Some((file, 1));
        Some(slot)
    }

    fn unuse_palette(&mut self, slot: usize) {
        if let Some((_, users)) = self.palettes[slot].as_mut() {
            *users = users.saturating_sub(1);
        }
    }

    /// Find the least data to move between free ranges that together satisfy `required`.
    /// The first hole moves right one adjacent resident at a time, merging holes as it goes.
    fn compaction_move(&self, required: u32) -> Option<(usize, u32, u32)> {
        if !self.fading_out.is_empty() {
            return None;
        }
        let (mut end, mut sum) = (0usize, 0u64);
        let mut best: Option<(u64, usize)> = None;
        for first in 0..self.free.len() {
            while end < self.free.len() && sum < u64::from(required) {
                sum += u64::from(self.free[end].1);
                end += 1;
            }
            if sum >= u64::from(required) && end > first {
                let (last_start, last_len) = self.free[end - 1];
                let span = u64::from(last_start + last_len - self.free[first].0);
                let moved = span - sum;
                if moved > 0 && best.is_none_or(|(cost, _)| moved < cost) {
                    best = Some((moved, first));
                }
            }
            sum = sum.saturating_sub(u64::from(self.free[first].1));
        }
        let (_, first) = best?;
        let (hole_start, hole_len) = self.free[first];
        let following = hole_start + hole_len;
        let leaf = self
            .leaves
            .iter()
            .position(|s| s.have.is_some_and(|h| h.start == following))?;
        Some((leaf, hole_start, hole_len))
    }

    /// Move one range per frame, preserving its palette and fade age. The old tail is culled
    /// only after the copy, including when source and destination overlap.
    fn compact_one(&mut self, pool: &GsplatScene, queue: &wgpu::Queue, required: u32) -> bool {
        let Some((leaf, hole_start, hole_len)) = self.compaction_move(required) else {
            return false;
        };
        let Some(mut resident) = self.leaves[leaf].have else {
            return false;
        };
        let old_start = resident.start;
        pool.move_slots(queue, old_start, hole_start, resident.count);
        let vacated_start = old_start.max(hole_start + resident.count);
        let old_end = old_start + resident.count;
        if vacated_start < old_end {
            pool.release(queue, vacated_start, old_end - vacated_start);
        }
        pool.write_lod(queue, hole_start, resident.count, resident.lod, leaf);
        self.take_exact(hole_start, hole_len);
        self.release(hole_start + resident.count, hole_len);
        resident.start = hole_start;
        self.leaves[leaf].have = Some(resident);
        true
    }

    /// First fit from the free list.
    fn allocate(&mut self, count: u32) -> Option<u32> {
        let index = self.free.iter().position(|&(_, len)| len >= count)?;
        let (start, len) = self.free[index];
        if len == count {
            self.free.remove(index);
        } else {
            self.free[index] = (start + count, len - count);
        }
        Some(start)
    }

    /// Takes exactly [start, start + count) out of the free list (it must be free).
    fn take_exact(&mut self, start: u32, count: u32) {
        let Some(index) = self
            .free
            .iter()
            .position(|&(s, len)| s <= start && start + count <= s + len)
        else {
            return;
        };
        let (s, len) = self.free.remove(index);
        let mut at = index;
        if s < start {
            self.free.insert(at, (s, start - s));
            at += 1;
        }
        if start + count < s + len {
            self.free
                .insert(at, (start + count, s + len - start - count));
        }
    }

    /// Returns a range to the free list, merging it with its neighbours.
    fn release(&mut self, start: u32, count: u32) {
        let at = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(at, (start, count));
        if at + 1 < self.free.len() && self.free[at].0 + self.free[at].1 == self.free[at + 1].0 {
            self.free[at].1 += self.free[at + 1].1;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].0 + self.free[at - 1].1 == self.free[at].0 {
            self.free[at - 1].1 += self.free[at].1;
            self.free.remove(at);
        }
    }

    /// One past the highest used slot.
    fn high_water(&self) -> u32 {
        match self.free.last() {
            Some(&(start, len)) if start + len == self.settings.capacity => start,
            _ => self.settings.capacity,
        }
    }

    /// Drops chunks no pending upload needs, keeping the `cache_chunks` most recently used.
    fn evict(&mut self) {
        // Even if every chunk were idle, none could be removed while the whole cache fits.
        // Avoid building the needed/prefetch sets and sorting on the common settled path.
        if self.chunks.len() <= self.settings.cache_chunks {
            return;
        }
        let mut needed = BTreeSet::new();
        for (state, leaf) in self.leaves.iter().zip(&self.meta.leaves) {
            if let Some(want) = state
                .want
                .filter(|&w| state.have.is_none_or(|h| h.lod != w))
            {
                if let Some(r) = leaf.lods[want] {
                    needed.insert(r.file);
                }
            }
        }
        needed.extend(self.prefetch().into_iter().map(|(_, file)| file));
        let mut idle: Vec<(u64, usize)> = self
            .chunks
            .iter()
            .filter(|(f, _)| !needed.contains(f))
            .map(|(f, c)| (c.last_used, *f))
            .collect();
        idle.sort();
        let excess = idle.len().saturating_sub(self.settings.cache_chunks);
        for (_, file) in idle.into_iter().take(excess) {
            self.chunks.remove(&file);
        }
    }
}

/// For an empty leaf, only use an available range at or coarser than its
/// selection. Finer fallbacks can overfill the pool before the target arrives.
fn loaded_fallback(
    lods: &[Option<Range>],
    want: usize,
    is_loaded: impl Fn(usize) -> bool,
) -> Option<usize> {
    (want..lods.len()).find(|&lod| lods[lod].is_some_and(|range| is_loaded(range.file)))
}

// Remove completed fades in their original order while keeping both buffers.
fn collect_completed_fades(
    fading: &mut Vec<(Resident, f32)>,
    completed: &mut Vec<Resident>,
    now: f32,
    fade_s: f32,
) {
    completed.clear();
    fading.retain(|&(resident, start)| {
        if now - start >= fade_s {
            completed.push(resident);
            false
        } else {
            true
        }
    });
}

// A coarse placeholder may cover an empty leaf while detail loads. Do not use
// a finer fallback, which can overflow the pool and disrupt smooth transitions.
fn loaded_range(
    lods: &[Option<Range>],
    want: usize,
    has_resident: bool,
    is_loaded: impl Fn(usize) -> bool,
) -> Option<(usize, Range)> {
    let lod = if lods[want].is_some_and(|range| is_loaded(range.file)) {
        Some(want)
    } else if !has_resident {
        loaded_fallback(lods, want, is_loaded)
    } else {
        None
    }?;
    Some((lod, lods[lod].expect("checked above")))
}

/// A new range must finish appearing before another transition may replace it.
fn resident_is_settled(resident: Resident, now: f32, fade_s: f32) -> bool {
    fade_s <= 0.0 || now - resident.born >= fade_s
}

/// If a newly appearing range leaves the draw region, continue from its actual
/// opacity instead of jumping to full opacity when retirement begins.
fn retirement_start(resident: Resident, now: f32, fade_s: f32) -> f32 {
    if fade_s <= 0.0 {
        return now;
    }
    let visible = ((now - resident.born) / fade_s).clamp(0.0, 1.0);
    now - (1.0 - visible) * fade_s
}

/// A leaf comes into view once it is within this many half-screens of the centre, and leaves it only past the wider
/// margin, so leaves at the edge of the view do not flip in and out as the camera sways. The entry margin matches the
/// depth-key pass's (`CULL_MARGIN` in `depth_keys.wgsl`): splats that far off screen are still drawn, as large ones
/// reach into the picture, so their leaves must count as seen.
const ENTER_MARGIN: f32 = 1.5;
const LEAVE_MARGIN: f32 = 1.8;
/// A leaf already showing the LOD it wants competes for the budget as if this much nearer, so leaves at the edge of
/// the budget keep their detail instead of trading it back and forth as the camera moves.
const STICKY: f32 = 0.7;
/// An automatic pool (`capacity` 0) holds this share of the level's LOD 0 splats, within these bounds. Measured on
/// the scans here: a pool of about 80 % of LOD 0 keeps the picture in front of the camera within about 0.1 % of full
/// detail (60 % left 0.4-0.5 % differing at 15-30 m on the denser scans).
const AUTO_POOL_SHARE: f64 = 0.8;
const AUTO_POOL_MIN: u32 = 3_000_000;
const AUTO_POOL_MAX: u32 = 8_000_000;

/// The share of the pool the selection may fill; the rest holds ranges while they crossfade.
const SELECT_SHARE: i64 = 85;

#[cfg(test)]
mod tests;
