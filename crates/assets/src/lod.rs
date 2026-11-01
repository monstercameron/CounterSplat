//! A streamed level's `lod-meta.json`, as splat-transform writes it: the chunk files of every level of detail.
//!
//! Chunk `L_c/` holds part of level of detail `L` (0 is the finest) as an unpacked SOG bundle. This engine draws one
//! whole level of detail: the finest whose splat count fits the budget.

/// The parts of `lod-meta.json` used here.
#[derive(Debug, Clone, PartialEq)]
pub struct LodMeta {
    /// Splats per level of detail, finest first.
    pub counts: Vec<u64>,
    /// Chunk `meta.json` paths relative to the level folder, e.g. `2_0/meta.json`. A leaf's `file` indexes this.
    pub filenames: Vec<String>,
    /// The tree's leaves: a box of splats, stored once per level of detail.
    pub leaves: Vec<Leaf>,
}

/// One leaf of the level's tree: its bounds (splat space) and, per level of detail (finest first), where its splats
/// are: a run of `count` splats from `offset` in chunk file `file`.
#[derive(Debug, Clone, PartialEq)]
pub struct Leaf {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub lods: Vec<Option<Range>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub file: usize,
    pub offset: u32,
    pub count: u32,
}

impl LodMeta {
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_slice(json).map_err(|e| format!("lod-meta.json: {e}"))?;
        let counts = value["counts"]
            .as_array()
            .ok_or("lod-meta.json: no counts")?
            .iter()
            .map(|v| v.as_u64().unwrap_or(0))
            .collect();
        let filenames = value["filenames"]
            .as_array()
            .ok_or("lod-meta.json: no filenames")?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let levels = value["lodLevels"].as_u64().unwrap_or(0) as usize;
        let mut leaves = Vec::new();
        collect_leaves(&value["tree"], levels, &mut leaves);
        Ok(Self {
            counts,
            filenames,
            leaves,
        })
    }

    /// The chunk folder of file index `file`, e.g. `2_0`.
    pub fn dir_of(&self, file: usize) -> Option<&str> {
        self.filenames.get(file).and_then(|f| f.split('/').next())
    }

    /// Levels of detail, finest (0) to coarsest.
    pub fn levels(&self) -> usize {
        self.counts.len()
    }

    /// The level of detail to draw. `requested` ≥ 0 picks that level (clamped to the coarsest). A negative value
    /// picks the finest level with at most `budget` splats, or the coarsest when none fits.
    pub fn pick(&self, requested: i32, budget: u64) -> u32 {
        let coarsest = self.counts.len().saturating_sub(1) as u32;
        if requested >= 0 {
            return (requested as u32).min(coarsest);
        }
        self.counts
            .iter()
            .position(|&n| n <= budget)
            .map_or(coarsest, |i| i as u32)
    }

    /// The chunk folders of level `lod`, e.g. `["2_0", "2_1", ...]`, in chunk order.
    pub fn chunk_dirs(&self, lod: u32) -> Vec<String> {
        let prefix = format!("{lod}_");
        let mut dirs: Vec<(u32, String)> = self
            .filenames
            .iter()
            .filter_map(|f| {
                let dir = f.split('/').next()?;
                let chunk = dir.strip_prefix(&prefix)?.parse().ok()?;
                Some((chunk, dir.to_string()))
            })
            .collect();
        dirs.sort();
        dirs.into_iter().map(|(_, d)| d).collect()
    }
}

fn collect_leaves(node: &serde_json::Value, levels: usize, out: &mut Vec<Leaf>) {
    if let Some(children) = node["children"].as_array() {
        for child in children {
            collect_leaves(child, levels, out);
        }
        return;
    }
    let vec3 = |v: &serde_json::Value| [0, 1, 2].map(|i| v[i].as_f64().unwrap_or(0.0) as f32);
    let lods = (0..levels)
        .map(|l| {
            let r = &node["lods"][l.to_string()];
            Some(Range {
                file: r["file"].as_u64()? as usize,
                offset: r["offset"].as_u64()? as u32,
                count: r["count"].as_u64()? as u32,
            })
        })
        .collect();
    out.push(Leaf {
        min: vec3(&node["bound"]["min"]),
        max: vec3(&node["bound"]["max"]),
        lods,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &[u8] = br#"{"counts":[1000,500,250],"filenames":["0_0/meta.json","2_0/meta.json","1_0/meta.json","0_1/meta.json","1_1/meta.json","0_10/meta.json","0_2/meta.json"]}"#;

    #[test]
    fn picks_the_finest_level_within_budget() {
        let meta = LodMeta::parse(META).unwrap();
        assert_eq!(meta.pick(-1, 600), 1);
        assert_eq!(meta.pick(-1, 5000), 0);
        assert_eq!(meta.pick(-1, 10), 2);
        assert_eq!(meta.pick(7, 0), 2);
        assert_eq!(meta.pick(1, 0), 1);
    }

    #[test]
    fn reads_the_tree_leaves_and_their_ranges() {
        let json = br#"{"counts":[30,10],"lodLevels":2,"filenames":["0_0/meta.json","1_0/meta.json"],
            "tree":{"bound":{"min":[0,0,0],"max":[2,1,1]},"children":[
              {"bound":{"min":[0,0,0],"max":[1,1,1]},"lods":{"0":{"file":0,"offset":0,"count":20},"1":{"file":1,"offset":0,"count":6}}},
              {"bound":{"min":[1,0,0],"max":[2,1,1]},"lods":{"0":{"file":0,"offset":20,"count":10},"1":{"file":1,"offset":6,"count":4}}}]}}"#;
        let meta = LodMeta::parse(json).unwrap();
        assert_eq!(meta.leaves.len(), 2);
        assert_eq!(
            meta.leaves[1].lods[0],
            Some(Range {
                file: 0,
                offset: 20,
                count: 10
            })
        );
        assert_eq!(
            meta.leaves[1].lods[1],
            Some(Range {
                file: 1,
                offset: 6,
                count: 4
            })
        );
        assert_eq!(meta.dir_of(1), Some("1_0"));
    }

    #[test]
    fn lists_chunks_in_numeric_order() {
        let meta = LodMeta::parse(META).unwrap();
        assert_eq!(meta.chunk_dirs(0), ["0_0", "0_1", "0_2", "0_10"]);
        assert_eq!(meta.chunk_dirs(1), ["1_0", "1_1"]);
    }
}
