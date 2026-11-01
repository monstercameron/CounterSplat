//! Configuration and diagnostics exposed by the streaming API.
use super::{LAYOUT_HASH_OFFSET, LAYOUT_HASH_PRIME};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamSettings {
    /// LOD 0 reaches this far from the camera, in level units (metres); each further LOD doubles it. Used only when
    /// `lod_error_px` is 0.
    pub lod_distance_m: f32,
    /// Screen-space error target: each leaf gets the coarsest LOD whose splats land at most this many pixels apart
    /// (at a 1080-pixel-tall view). Dense scans then use coarse levels where they look the same, and sparse ones keep
    /// fine levels further out. 0 picks by `lod_distance_m` instead.
    pub lod_error_px: f32,
    /// Pool slots: the most splats resident at once. 0 sizes the pool to the level (`AUTO_POOL_SHARE` of its LOD 0
    /// splats, between `AUTO_POOL_MIN` and `AUTO_POOL_MAX`): a dense scan needs more room for the same picture.
    pub capacity: u32,
    /// Leaves farther than this from the camera are not drawn at all (a draw distance), in level units.
    pub max_distance_m: f32,
    /// Extra LODs for leaves outside the view (behind the camera, or off to the side when the view is known).
    pub behind_bias: usize,
    /// Splats re-encoded and uploaded per frame, at most.
    pub uploads_per_frame: u32,
    /// LODs finer than this keep their spherical harmonics.
    pub sh_lods: usize,
    /// Decoded chunk files kept in memory once nothing pending needs them.
    pub cache_chunks: usize,
    /// Chunk files requested but not yet inserted, at most.
    pub max_in_flight: usize,
    /// Seconds a change of LOD crossfades over (0 swaps at once).
    pub fade_s: f32,
    /// Seconds a leaf keeps its finer LOD after its target first goes coarser.
    pub keep_s: f32,
    /// Leaves this close to the LOD centre count as in view whichever way the camera looks: the chase camera swings
    /// around the focus, and nearby detail remains available during camera movement.
    pub any_view_m: f32,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            lod_distance_m: 24.0,
            lod_error_px: 1.5,
            max_distance_m: 150.0,
            capacity: 3_000_000,
            behind_bias: 1,
            uploads_per_frame: 250_000,
            sh_lods: 1,
            cache_chunks: 8,
            max_in_flight: 3,
            fade_s: 0.35,
            keep_s: 1.5,
            any_view_m: 8.0,
        }
    }
}

/// One resident leaf for the debug layer: its box (splat space), the LOD it shows, whether it is waiting for a
/// different one, and whether it is in view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugLeaf {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub lod: usize,
    pub pending: bool,
    pub visible: bool,
}

/// What the stream holds right now, for status lines and the console.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamStats {
    pub resident_splats: u32,
    /// Splats the current selection asks for (resident once nothing is pending).
    pub wanted_splats: u64,
    /// The highest used pool slot: what the sort covers.
    pub active_slots: u32,
    pub leaves_per_lod: Vec<u32>,
    /// Leaves waiting for their first data.
    pub leaves_empty: u32,
    /// Leaves past the draw distance.
    pub leaves_hidden: u32,
    pub leaves_pending: u32,
    pub chunks_loaded: usize,
    pub chunks_in_flight: usize,
    /// Leaves in the view, and ranges crossfading now.
    pub leaves_visible: u32,
    pub fading: u32,
    /// Changes of LOD so far (uploads replacing a resident range), and how many of those were leaves in view
    /// within `lod_distance_m` (the swaps most likely to be seen).
    pub switches: u64,
    pub near_switches: u64,
    /// SH palette slots in use, and resident leaves fine enough for SH whose chunk has it but got no palette.
    pub palettes_in_use: u32,
    pub missing_sh: u32,
    /// Times crossfades were cut short for room, and leaves swapped without one (released before the upload).
    pub fades_cut: u64,
    pub hard_swaps: u64,
    /// The share of drawn leaves at the LOD their distance asks for (1 when the pool holds all the detail wanted).
    pub detail_met: f32,
    /// How far `lod_distance_m` is scaled to fit the pool (1: the full distance).
    pub detail_scale: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResidentRangeLayout {
    pub leaf: usize,
    pub file: usize,
    pub lod: usize,
    pub source_offset: u32,
    pub count: u32,
    pub pool_start: u32,
    pub palette: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutAllocationEvent {
    pub frame: u64,
    pub time_s: f32,
    pub leaf: usize,
    pub file: usize,
    pub lod: usize,
    pub source_offset: u32,
    pub count: u32,
    pub pool_start: u32,
    pub palette: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamLayoutDiagnostics {
    pub enabled_from_frame: u64,
    pub resident_fingerprint: String,
    pub allocation_fingerprint: String,
    pub allocation_count: u64,
    pub allocation_events_dropped: u64,
    pub resident_ranges: Vec<ResidentRangeLayout>,
    pub allocation_history: Vec<LayoutAllocationEvent>,
}

impl StreamLayoutDiagnostics {
    pub fn json(&self) -> serde_json::Value {
        let resident_ranges = self
            .resident_ranges
            .iter()
            .map(|r| {
                serde_json::json!({
                    "leaf": r.leaf, "file": r.file, "lod": r.lod, "source_offset": r.source_offset,
                    "count": r.count, "pool_start": r.pool_start, "palette": r.palette,
                })
            })
            .collect::<Vec<_>>();
        let allocation_history = self
            .allocation_history
            .iter()
            .map(|e| {
                serde_json::json!({
                    "frame": e.frame, "time_s": e.time_s, "leaf": e.leaf, "file": e.file,
                    "lod": e.lod, "source_offset": e.source_offset, "count": e.count,
                    "pool_start": e.pool_start, "palette": e.palette,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "enabled_from_frame": self.enabled_from_frame,
            "resident_mapping_scope": "current_leaf_residents_excluding_fading_out",
            "resident_fingerprint": self.resident_fingerprint,
            "allocation_fingerprint": self.allocation_fingerprint,
            "allocation_count": self.allocation_count,
            "allocation_events_dropped": self.allocation_events_dropped,
            "resident_ranges": resident_ranges,
            "allocation_history": allocation_history,
        })
    }
}

impl StreamStats {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "resident_splats": self.resident_splats,
            "wanted_splats": self.wanted_splats,
            "active_slots": self.active_slots,
            "leaves_per_lod": self.leaves_per_lod,
            "leaves_empty": self.leaves_empty,
            "leaves_hidden": self.leaves_hidden,
            "leaves_pending": self.leaves_pending,
            "chunks_loaded": self.chunks_loaded,
            "chunks_in_flight": self.chunks_in_flight,
            "leaves_visible": self.leaves_visible,
            "fading": self.fading,
            "switches": self.switches,
            "near_switches": self.near_switches,
            "palettes_in_use": self.palettes_in_use,
            "missing_sh": self.missing_sh,
            "fades_cut": self.fades_cut,
            "hard_swaps": self.hard_swaps,
            "detail_met": self.detail_met,
            "detail_scale": self.detail_scale,
        })
    }
}

fn hash_layout_value(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(LAYOUT_HASH_PRIME);
    }
}

pub(super) fn hash_resident_layout(entries: &[ResidentRangeLayout]) -> u64 {
    let mut hash = LAYOUT_HASH_OFFSET;
    hash_layout_value(&mut hash, entries.len() as u64);
    for entry in entries {
        for value in [
            entry.leaf as u64,
            entry.file as u64,
            entry.lod as u64,
            u64::from(entry.source_offset),
            u64::from(entry.count),
            u64::from(entry.pool_start),
            entry.palette.map_or(u64::MAX, |palette| palette as u64),
        ] {
            hash_layout_value(&mut hash, value);
        }
    }
    hash
}

pub(super) fn hash_allocation_event(hash: &mut u64, event: &LayoutAllocationEvent) {
    for value in [
        event.frame,
        u64::from(event.time_s.to_bits()),
        event.leaf as u64,
        event.file as u64,
        event.lod as u64,
        u64::from(event.source_offset),
        u64::from(event.count),
        u64::from(event.pool_start),
        event.palette.map_or(u64::MAX, |palette| palette as u64),
    ] {
        hash_layout_value(hash, value);
    }
}
