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

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];
/// Identity with the z axis flipped, so the depth is -z and the order reverses.
const FLIPPED: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The sort ignores the low key bits (about 0.05% of the depth), so it matches a stable CPU sort except among
/// depths that close. Checks `order` is a permutation whose depths ascend to within that tolerance.
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
    // Sizes around the 2048-element workgroup boundary, plus a multi-level scan case.
    for count in [1u32, 2, 255, 2047, 2048, 2049, 9000, 70_001] {
        let z = depths(count, count);
        // Centres with x = y = 0, so only z matters; the depth keys read m[2] of the identity model.
        let centers: Vec<f32> = z.iter().flat_map(|&z| [0.0, 0.0, z]).collect();
        let Some(gpu) = sort_on_gpu(count, &centers, &[IDENTITY]) else {
            eprintln!("no GPU adapter available; skipping sort test");
            return;
        };
        assert_sorted(&gpu[0], |i| z[i as usize], &format!("count {count}"));
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
    assert_sorted(&results[0], |i| z[i as usize], "identity");
    assert_sorted(&results[2], |i| -z[i as usize], "flipped");
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
    assert_eq!(args[0], per_instance * 6, "six indices per Gaussian");
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
