//! Planning for a pool of single-threaded decode workers (the browser's Web Workers): how many to start, how a
//! level with fewer chunks than workers is split into per-stream jobs and merged back, and how much decoded work
//! to hand the renderer in one frame. Pure logic with no platform types, so it is tested natively.

use std::collections::VecDeque;

/// Most decode workers a host starts. Each decode is single-threaded; on a 20-thread machine 24 level chunks took
/// 1755 ms on one worker, 630 ms on four, 473 ms on eight and 423 ms on twelve.
pub const MAX_DECODERS: u32 = 8;

/// Streams per chunk (means_l, means_u, quats, scales, sh0, SH labels, SH centroids).
pub const STREAMS: usize = 7;

/// Stream groups a chunk is split into when there are more workers than chunks. The two SH streams are usually
/// small and go together.
pub const SPLIT_GROUPS: &[&[usize]] = &[&[0], &[1], &[2], &[3], &[4], &[5, 6]];

/// Decode workers for `logical_cores` (unknown when `None`), or the scene's explicit `configured` count (1 to
/// `MAX_DECODERS`).
pub fn decoder_count(logical_cores: Option<u32>, configured: Option<u32>) -> Result<u32, String> {
    if let Some(count) = configured {
        if !(1..=MAX_DECODERS).contains(&count) {
            return Err(format!(
                "Chunk decoder workers must be auto or an integer from 1 to {MAX_DECODERS}."
            ));
        }
        return Ok(count);
    }
    Ok(match logical_cores {
        None | Some(0) => 2,
        Some(1..=2) => 1,
        Some(3..=4) => 2,
        Some(5..=8) => 3,
        Some(9..=12) => 4,
        Some(13..=16) => 6,
        Some(_) => MAX_DECODERS,
    })
}

/// One unit of worker work: a whole chunk (`parts` is `None`) or some of its streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub chunk: usize,
    pub parts: Option<&'static [usize]>,
}

/// The jobs for `chunks` chunks on `workers` workers, in chunk order. A level with fewer chunks than workers (a
/// single-file scan is one chunk) would leave workers idle, so each chunk is split into `SPLIT_GROUPS`.
pub fn plan_jobs(chunks: usize, workers: u32) -> Vec<Job> {
    let split = chunks < workers as usize;
    (0..chunks)
        .flat_map(|chunk| {
            let groups: Vec<Option<&'static [usize]>> = if split {
                SPLIT_GROUPS.iter().map(|group| Some(*group)).collect()
            } else {
                vec![None]
            };
            groups.into_iter().map(move |parts| Job { chunk, parts })
        })
        .collect()
}

/// Collects the pieces of split chunks. `T` is one decoded stream.
#[derive(Debug)]
pub struct Merger<T> {
    partial: Vec<Option<(Vec<Option<T>>, usize)>>,
}

impl<T> Merger<T> {
    pub fn new(chunks: usize) -> Self {
        Self {
            partial: (0..chunks).map(|_| None).collect(),
        }
    }

    /// Adds a job's result: `streams` holds `STREAMS` entries, of which the job's `parts` (or all of them, for a
    /// whole chunk) are filled. Returns the chunk's streams once every piece has arrived.
    pub fn add(&mut self, job: &Job, mut streams: Vec<Option<T>>) -> Option<Vec<T>> {
        let Some(parts) = job.parts else {
            return streams.into_iter().collect();
        };
        let pieces = SPLIT_GROUPS.len();
        let entry = self.partial[job.chunk]
            .get_or_insert_with(|| ((0..STREAMS).map(|_| None).collect(), 0));
        for &part in parts {
            entry.0[part] = streams.get_mut(part).and_then(Option::take);
        }
        entry.1 += 1;
        if entry.1 < pieces {
            return None;
        }
        let (merged, _) = self.partial[job.chunk].take()?;
        merged.into_iter().collect()
    }
}

/// Hands queued items to `insert` until `budget_ms` has passed (`now` is a millisecond clock), always at least one.
/// Copying a decoded chunk into the renderer takes a few milliseconds, and workers finish in bursts, so spreading
/// them over frames keeps a burst from stalling one frame.
pub fn drain_within<T>(
    queue: &mut VecDeque<T>,
    budget_ms: f64,
    mut now: impl FnMut() -> f64,
    mut insert: impl FnMut(T),
) -> usize {
    let start = now();
    let mut done = 0;
    while let Some(item) = queue.pop_front() {
        insert(item);
        done += 1;
        if now() - start >= budget_ms {
            break;
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_counts_follow_cores_and_validate_configuration() {
        let counts: Vec<u32> = [Some(1), Some(2), Some(4), Some(8), Some(12), Some(16), Some(20), Some(64), None]
            .into_iter()
            .map(|cores| decoder_count(cores, None).unwrap())
            .collect();
        assert_eq!(counts, [1, 1, 2, 3, 4, 6, 8, 8, 2]);
        assert_eq!(decoder_count(Some(16), Some(1)), Ok(1));
        assert_eq!(decoder_count(Some(1), Some(8)), Ok(8));
        assert!(decoder_count(Some(8), Some(0)).is_err());
        assert!(decoder_count(Some(8), Some(9)).is_err());
    }

    #[test]
    fn scarce_chunks_split_into_stream_groups() {
        assert_eq!(plan_jobs(5, 3), (0..5).map(|chunk| Job { chunk, parts: None }).collect::<Vec<_>>());
        let jobs = plan_jobs(1, 3);
        assert_eq!(jobs.len(), SPLIT_GROUPS.len());
        assert!(jobs.iter().all(|job| job.chunk == 0));
        let mut covered: Vec<usize> = jobs.iter().flat_map(|job| job.parts.unwrap().iter().copied()).collect();
        covered.sort_unstable();
        assert_eq!(covered, (0..STREAMS).collect::<Vec<_>>(), "every stream exactly once");
    }

    #[test]
    fn split_pieces_merge_in_any_order() {
        let jobs = plan_jobs(1, 8);
        let mut merger = Merger::new(1);
        let piece = |job: &Job| -> Vec<Option<usize>> {
            (0..STREAMS).map(|s| job.parts.unwrap().contains(&s).then_some(s * 10)).collect()
        };
        let order = [5, 2, 0, 4, 1];
        for &i in &order {
            assert_eq!(merger.add(&jobs[i], piece(&jobs[i])), None);
        }
        assert_eq!(merger.add(&jobs[3], piece(&jobs[3])), Some(vec![0, 10, 20, 30, 40, 50, 60]));
        // Whole-chunk jobs pass straight through.
        let mut whole = Merger::new(2);
        let job = Job { chunk: 1, parts: None };
        assert_eq!(whole.add(&job, (0..STREAMS).map(Some).collect()), Some((0..STREAMS).collect()));
    }

    #[test]
    fn draining_stops_after_the_budget_but_takes_at_least_one() {
        let mut queue: VecDeque<u32> = (0..5).collect();
        let mut clock = 0.0;
        let mut inserted = Vec::new();
        // The clock advances 1 ms a reading; the budget runs out at the third check.
        let done = drain_within(&mut queue, 3.0, || { let t = clock; clock += 1.0; t }, |item| inserted.push(item));
        assert_eq!((done, inserted.as_slice()), (3, &[0, 1, 2][..]));
        assert_eq!(queue.len(), 2);
        let mut slow = VecDeque::from([7]);
        let mut t = 0.0;
        assert_eq!(drain_within(&mut slow, 0.0, || { t += 100.0; t }, |_| {}), 1);
    }
}
