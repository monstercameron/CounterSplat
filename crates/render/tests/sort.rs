//! The GPU radix sort must order splats exactly as a stable CPU sort by view-space depth.

use countersplat_render::sort::DepthSorter;
use countersplat_render::try_device;

/// Deterministic pseudo-random depths in [-100, 100), distinct enough that ties are rare.
fn depths(count: u32, seed: u32) -> Vec<f32> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) * 200.0 - 100.0
        })
        .collect()
}

/// Sorts `centers` once per matrix in `view_models`, in order, on one sorter and one output buffer, returning each
/// result. Repeating a matrix exercises the sorter's skip of an unchanged sort.
fn sort_on_gpu(count: u32, centers: &[f32], view_models: &[[f32; 16]]) -> Option<Vec<Vec<u32>>> {
    let (device, queue) = try_device()?;
    let centers_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test centres"),
        size: centers.len() as u64 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &centers_buffer,
        0,
        &centers
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let fade = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test fade"),
        size: u64::from(count) * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let sorter = DepthSorter::new(&device, &queue, count, &centers_buffer, &fade);

    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test output"),
        size: u64::from(count) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut results = Vec::new();
    for view_model in view_models {
        let mut encoder = device.create_command_encoder(&Default::default());
        sorter.encode(&queue, &mut encoder, view_model, None, None, &output);
        queue.submit(Some(encoder.finish()));

        let slice = output.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let mapped = slice.get_mapped_range().ok()?;
        results.push(
            mapped
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        );
        drop(mapped);
        output.unmap();
    }
    Some(results)
}

/// The camera 200 units back along +z, so every test depth (z in [-100, 100)) is in front of it: view z = z - 200.
/// The sort orders splats in front of the camera; those behind share one key (a clip test culls them anyway).
const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -200.0, 1.0,
];
/// The z axis flipped, so view z = -z - 200 and the order reverses.
const FLIPPED: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -200.0, 1.0,
];

/// The key is a 16-bit logarithmic depth (a step is about 0.03% of the depth), so the sort matches a stable CPU sort
/// except among depths that close. `depth` is the view-space z. Checks `order` is a permutation whose depths ascend to within that tolerance.
fn assert_sorted(order: &[u32], depth: impl Fn(u32) -> f32, what: &str) {
    let mut seen = vec![false; order.len()];
    for &i in order {
        assert!(
            !std::mem::replace(&mut seen[i as usize], true),
            "{what}: index {i} appears twice"
        );
    }
    for pair in order.windows(2) {
        let (a, b) = (depth(pair[0]), depth(pair[1]));
        assert!(
            a <= b + 1e-3 * a.abs().max(b.abs()) + f32::MIN_POSITIVE,
            "{what}: depth {a} sorted before {b}"
        );
    }
}

#[test]
fn gpu_sort_matches_cpu_order_to_key_precision() {
    // Sizes around the 2048-element workgroup boundary, a multi-level scan case, and a million splats.
    for count in [1u32, 2, 255, 2047, 2048, 2049, 9000, 70_001, 1_000_003] {
        let z = depths(count, count);
        // Centres with x = y = 0, so only z matters; the depth keys read m[2] of the identity model.
        let centers: Vec<f32> = z.iter().flat_map(|&z| [0.0, 0.0, z]).collect();
        let Some(gpu) = sort_on_gpu(count, &centers, &[IDENTITY]) else {
            eprintln!("no GPU adapter available; skipping sort test");
            return;
        };
        assert_sorted(&gpu[0], |i| z[i as usize] - 200.0, &format!("count {count}"));
    }
}

#[test]
fn sort_is_stable_for_equal_depths() {
    // Every splat at one of four depths: equal keys must keep their original (index) order, which the draw's
    // blend order and a crossfade's slot ranges rely on.
    let count = 50_000u32;
    let z: Vec<f32> = (0..count).map(|i| (i % 4) as f32 * 3.0 - 6.0).collect();
    let centers: Vec<f32> = z.iter().flat_map(|&z| [0.0, 0.0, z]).collect();
    let Some(gpu) = sort_on_gpu(count, &centers, &[IDENTITY]) else {
        eprintln!("no GPU adapter available; skipping sort test");
        return;
    };
    for pair in gpu[0].windows(2) {
        let (a, b) = (pair[0] as usize, pair[1] as usize);
        assert!(
            z[a] < z[b] || (z[a] == z[b] && a < b),
            "{a} (depth {}) sorted before {b} (depth {})",
            z[a],
            z[b]
        );
    }
}

#[test]
fn unchanged_view_model_keeps_the_order_and_a_changed_one_resorts() {
    let count = 9000;
    let z = depths(count, 7);
    let centers: Vec<f32> = z.iter().flat_map(|&z| [0.0, 0.0, z]).collect();
    // The repeat is skipped by the sorter; the flip and the return to identity must each sort afresh.
    let Some(results) = sort_on_gpu(
        count,
        &centers,
        &[IDENTITY, IDENTITY, FLIPPED, FLIPPED, IDENTITY],
    ) else {
        eprintln!("no GPU adapter available; skipping sort test");
        return;
    };
    assert_eq!(
        results[0], results[1],
        "an unchanged matrix must reproduce the order"
    );
    assert_eq!(
        results[0], results[4],
        "returning to the first matrix must reproduce its order"
    );
    assert_eq!(results[2], results[3]);
    assert_sorted(&results[0], |i| z[i as usize] - 200.0, "identity");
    assert_sorted(&results[2], |i| -z[i as usize] - 200.0, "flipped");
}

#[test]
fn empty_slots_are_not_drawn() {
    // A streaming pool empties released slots by giving them NaN centres. They must be left out of the draw: every
    // comparison with NaN is false, so a cull written as `w <= 0` alone would let them through.
    let count = 5000u32;
    let centers: Vec<f32> = (0..count)
        .flat_map(|i| {
            if i % 7 == 0 {
                [f32::NAN; 3]
            } else {
                [0.0, 0.0, -(i as f32) * 0.01 - 1.0]
            }
        })
        .collect();
    let real = (0..count).filter(|i| i % 7 != 0).count() as u32;
    let Some((device, queue)) = try_device() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };
    let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let centres_buffer = buffer(
        "centres",
        u64::from(count) * 12,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    queue.write_buffer(
        &centres_buffer,
        0,
        &centers
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let fade = buffer("fade", u64::from(count) * 4, wgpu::BufferUsages::STORAGE);
    let sorter = DepthSorter::new(&device, &queue, count, &centres_buffer, &fade);
    let order = buffer("order", u64::from(count) * 4, wgpu::BufferUsages::COPY_DST);
    let readback = buffer(
        "args",
        20,
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    sorter.encode(&queue, &mut encoder, &IDENTITY, None, None, &order);
    encoder.copy_buffer_to_buffer(sorter.draw_args(), 0, &readback, 0, 20);
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let args: Vec<u32> = slice
        .get_mapped_range()
        .expect("mapped")
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let per_instance = countersplat_render::scene::INSTANCE_SIZE;
    assert_eq!(
        args[0],
        per_instance * countersplat_render::scene::INDICES_PER_SPLAT,
        "two triangles (six indices) per Gaussian"
    );
    assert_eq!(
        &args[2..],
        &[0, 0, 0],
        "first index, base vertex and first instance must start at zero"
    );
    assert_eq!(
        args[1],
        real.div_ceil(per_instance),
        "instances drawn cover only the {real} real splats, not the empty slots"
    );
}

#[test]
fn unchanged_fades_reuse_the_sort_and_changed_fades_invalidate_it() {
    let Some((device, queue)) = try_device() else {
        return;
    };
    let count = 4;
    let buffer = |label, size, usage| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let centres = buffer(
        "fade test centres",
        count * 12,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    queue.write_buffer(
        &centres,
        0,
        bytemuck::cast_slice(&[
            0.0f32, 0.0, -1.0, 0.0, 0.0, -2.0, 0.0, 0.0, -3.0, 0.0, 0.0, -4.0,
        ]),
    );
    let fade = buffer("fade test values", count * 4, wgpu::BufferUsages::STORAGE);
    let output = buffer("fade test order", count * 4, wgpu::BufferUsages::COPY_DST);
    let sorter = DepthSorter::new(&device, &queue, count as u32, &centres, &fade);
    let render = |ranges: &[(u32, u32, f32)]| {
        sorter.set_fades(&queue, ranges);
        let mut encoder = device.create_command_encoder(&Default::default());
        sorter.encode(&queue, &mut encoder, &IDENTITY, None, None, &output);
        queue.submit(Some(encoder.finish()));
    };
    // Before the first sort a share-only change is impossible; the first sort writes everything.
    render(&[(0, 4, 0.5)]);
    render(&[(0, 4, 0.5)]);
    assert_eq!(sorter.profile_json()["counters"]["sorts"], 1);
    assert_eq!(sorter.profile_json()["counters"]["cache_hits"], 1);
    // Only a share changed and it stays above zero: the order holds and only the shares are rewritten.
    render(&[(0, 4, 0.6)]);
    assert_eq!(sorter.profile_json()["counters"]["sorts"], 1);
    assert_eq!(sorter.profile_json()["counters"]["fade_updates"], 1);
    // A share reaching zero leaves the range out of the draw, and a removed range changes it: both sort.
    render(&[(0, 4, 0.0)]);
    render(&[]);
    render(&[]);
    assert_eq!(sorter.profile_json()["counters"]["sorts"], 3);
    assert_eq!(sorter.profile_json()["counters"]["cache_hits"], 2);
}

/// Rewriting only the crossfade shares (camera unchanged, no share crossing zero) must leave exactly what a full
/// sort with the new shares writes: the same order and the same per-slot opacity.
#[test]
fn share_only_fade_updates_match_a_full_sort() {
    let Some((device, queue)) = try_device() else {
        return;
    };
    let count = 5000u32;
    let z = depths(count, 11);
    let centers: Vec<f32> = z.iter().flat_map(|&z| [0.0, 0.0, z]).collect();
    let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    let buffer = |label: &str, size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: storage,
            mapped_at_creation: false,
        })
    };
    let centres = buffer("share test centres", u64::from(count) * 12);
    queue.write_buffer(&centres, 0, bytemuck::cast_slice(&centers));
    let fades = [buffer("updated fade", u64::from(count) * 4), buffer("sorted fade", u64::from(count) * 4)];
    let orders = [buffer("updated order", u64::from(count) * 4), buffer("sorted order", u64::from(count) * 4)];
    let sorters = [
        DepthSorter::new(&device, &queue, count, &centres, &fades[0]),
        DepthSorter::new(&device, &queue, count, &centres, &fades[1]),
    ];
    let ranges = |share: f32| vec![(10, 900, share), (1500, 1600, 0.0), (2000, 4000, 1.0 - share)];
    let step = |sorter: &DepthSorter, order: &wgpu::Buffer, share: f32| {
        sorter.set_fades(&queue, &ranges(share));
        let mut encoder = device.create_command_encoder(&Default::default());
        sorter.encode(&queue, &mut encoder, &IDENTITY, None, None, order);
        queue.submit(Some(encoder.finish()));
    };
    step(&sorters[0], &orders[0], 0.3);
    step(&sorters[0], &orders[0], 0.7);
    step(&sorters[1], &orders[1], 0.7);
    assert_eq!(sorters[0].profile_json()["counters"]["fade_updates"], 1);
    let bytes = u64::from(count) * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("share test readback"),
        size: bytes * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    for (i, source) in [&orders[0], &orders[1], &fades[0], &fades[1]].into_iter().enumerate() {
        encoder.copy_buffer_to_buffer(source, 0, &readback, bytes * i as u64, bytes);
    }
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().unwrap();
    let n = bytes as usize;
    // Every slot is in view here (no clip test), so the whole order is defined.
    assert_eq!(&data[..n], &data[n..2 * n], "order");
    assert_eq!(&data[2 * n..3 * n], &data[3 * n..], "fade shares");
    let share = |slot: usize| f32::from_le_bytes(data[2 * n + slot * 4..2 * n + slot * 4 + 4].try_into().unwrap());
    assert_eq!((share(10), share(1500), share(3000), share(4500)), (0.7, 0.0, 1.0 - 0.7, 1.0));
}
