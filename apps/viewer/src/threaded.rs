//! Native viewer orchestration. Asset decoding and GPU state live on different owner threads.
//!
//! The command-line host waits for this one-shot render; an interactive host should poll tickets instead of
//! blocking its platform event loop. The renderer can request chunks from the independent asset worker, so the
//! asset worker must stay alive until rendering has finished.

use super::{load_lod, load_sog, Result};
use countersplat_assets::{lod::LodMeta, sog, viewer::CameraHint};
use countersplat_core::{
    pacing::{AdaptivePacer, WorkSample},
    rate::Rate,
    scheduler::Scheduler,
    time::FixedClock,
};
use countersplat_render::scene::Pipelines;
use countersplat_runtime::native::{Client, Ticket, Worker};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const QUEUE_CAPACITY: usize = 8;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct LevelData {
    pub meta: LodMeta,
    pub metas: Vec<Option<Vec<u8>>>,
    pub hint: Option<CameraHint>,
}

pub(super) enum AssetCommand {
    Sog(PathBuf),
    Metadata(PathBuf),
    Level(PathBuf),
    Sky(PathBuf),
}

pub(super) enum AssetReply {
    Sog(sog::Bundle),
    Metadata(LodMeta),
    Level(LevelData),
    Sky(countersplat_render::env::Cubemap),
}

pub(super) type AssetClient = Client<AssetCommand, AssetReply>;
pub(super) type AssetWorker = Worker<AssetCommand, AssetReply>;

pub(super) fn start_assets() -> Result<AssetWorker> {
    let mut worker = Worker::spawn(
        "countersplat-assets",
        QUEUE_CAPACITY,
        || Ok(()),
        |_, command| {
            let result: Result<AssetReply> = (|| match command {
                AssetCommand::Sog(path) => Ok(AssetReply::Sog(load_sog(&path)?)),
                AssetCommand::Metadata(path) => Ok(AssetReply::Metadata(load_lod(&path)?)),
                AssetCommand::Sky(path) => {
                    let panorama = countersplat_render::env::load_hdr(&path)?;
                    let cubemap = countersplat_render::env::try_to_cubemap(&panorama, 256, 0.0)?;
                    Ok(AssetReply::Sky(cubemap))
                }
                AssetCommand::Level(path) => {
                    let meta = load_lod(&path)?;
                    let metas = meta
                        .filenames
                        .iter()
                        .map(|name| std::fs::read(path.join(name)).ok())
                        .collect();
                    let hint = std::fs::read(path.join("viewer-settings.json"))
                        .ok()
                        .and_then(|bytes| CameraHint::from_json(&bytes).ok());
                    Ok(AssetReply::Level(LevelData { meta, metas, hint }))
                }
            })();
            result.map_err(|error| error.to_string())
        },
    )?;
    let thread = worker.wait_ready(STARTUP_TIMEOUT)?;
    println!("Asset worker: {thread:?}");
    Ok(worker)
}

pub(super) fn request(client: &AssetClient, command: AssetCommand) -> Result<AssetReply> {
    let ticket = client
        .try_submit(command)
        .map_err(|error| error.to_string())?;
    Ok(ticket.wait()?)
}

pub(super) fn load_chunk(client: &AssetClient, path: PathBuf) -> Result<sog::Bundle> {
    match request(client, AssetCommand::Sog(path))? {
        AssetReply::Sog(bundle) => Ok(bundle),
        _ => Err("asset worker returned an unexpected reply".into()),
    }
}

fn with_assets<T>(work: impl FnOnce(&AssetClient) -> Result<T>) -> Result<T> {
    let worker = start_assets()?;
    let outcome = work(&worker.client());
    let shutdown = worker.shutdown();
    let value = outcome?;
    shutdown?;
    Ok(value)
}

pub(super) fn inspect_sog(input: &Path) -> Result<()> {
    with_assets(|assets| {
        let bundle = load_chunk(assets, input.to_owned())?;
        let centers = sog::centres(&bundle);
        println!(
            "SOG v2: {} splats; {} decoded streams; center {:?}",
            bundle.meta.count,
            bundle.textures.len(),
            sog::mean_centre(&centers)
        );
        Ok(())
    })
}

pub(super) fn inspect_level(input: &Path) -> Result<()> {
    with_assets(|assets| {
        let meta = match request(assets, AssetCommand::Metadata(input.to_owned()))? {
            AssetReply::Metadata(meta) => meta,
            _ => return Err("asset worker returned an unexpected reply".into()),
        };
        println!(
            "Level: {} leaves; {} chunks; LOD splat counts {:?}",
            meta.leaves.len(),
            meta.filenames.len(),
            meta.counts
        );
        Ok(())
    })
}

pub(super) struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub pipelines: Pipelines,
    pub(super) level: Option<super::LoadedLevel>,
}

enum RenderCommand {
    Sog {
        bundle: sog::Bundle,
        output: PathBuf,
    },
    Level {
        data: LevelData,
        input: PathBuf,
        output: PathBuf,
        capacity: u32,
        rotate_z_180: bool,
    },
    PrepareLevel {
        data: LevelData,
        input: PathBuf,
        capacity: u32,
        rotate_z_180: bool,
    },
    LiveFrame {
        output: PathBuf,
        simulation_seconds: f64,
    },
}

type RenderClient = Client<RenderCommand, ()>;

fn with_renderer(work: impl FnOnce(&AssetClient, &RenderClient) -> Result<()>) -> Result<()> {
    let assets = start_assets()?;
    let chunk_client = assets.client();
    let mut renderer = Worker::spawn(
        "countersplat-rendering",
        QUEUE_CAPACITY,
        || {
            let (device, queue) = super::device().map_err(|error| error.to_string())?;
            let pipelines = Pipelines::new(&device, countersplat_render::COLOR_FORMAT)
                .map_err(|error| error.to_string())?;
            Ok(Gpu {
                device,
                queue,
                pipelines,
                level: None,
            })
        },
        move |gpu, command| {
            let result = match command {
                RenderCommand::Sog { bundle, output } => {
                    super::render_sog(bundle, &output, &gpu.device, &gpu.queue, &gpu.pipelines)
                }
                RenderCommand::Level {
                    data,
                    input,
                    output,
                    capacity,
                    rotate_z_180,
                } => super::render_level(
                    data,
                    &input,
                    &output,
                    capacity,
                    rotate_z_180,
                    &chunk_client,
                    gpu,
                ),
                RenderCommand::PrepareLevel {
                    data,
                    input,
                    capacity,
                    rotate_z_180,
                } => {
                    match super::prepare_level(
                        data,
                        &input,
                        capacity,
                        rotate_z_180,
                        &chunk_client,
                        gpu,
                    ) {
                        Ok(level) => {
                            gpu.level = Some(level);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                RenderCommand::LiveFrame {
                    output,
                    simulation_seconds,
                } => match &gpu.level {
                    Some(level) => super::draw_live_level(level, gpu, &output, simulation_seconds),
                    None => Err("no streamed scene is prepared".into()),
                },
            };
            result.map_err(|error| error.to_string())
        },
    )?;
    let outcome = (|| {
        let thread = renderer.wait_ready(STARTUP_TIMEOUT)?;
        println!("Render worker: {thread:?}");
        work(&assets.client(), &renderer.client())
    })();
    // Finish rendering before stopping assets: a render may still be waiting for a chunk reply.
    let render_shutdown = renderer.shutdown();
    let asset_shutdown = assets.shutdown();
    outcome?;
    render_shutdown?;
    asset_shutdown?;
    Ok(())
}

fn render(client: &RenderClient, command: RenderCommand) -> Result<()> {
    let ticket = client
        .try_submit(command)
        .map_err(|error| error.to_string())?;
    Ok(ticket.wait()?)
}

pub(super) fn render_sog(input: &Path, output: &Path) -> Result<()> {
    with_renderer(|assets, renderer| {
        let bundle = load_chunk(assets, input.to_owned())?;
        render(
            renderer,
            RenderCommand::Sog {
                bundle,
                output: output.to_owned(),
            },
        )
    })
}

pub(super) fn render_level(
    input: &Path,
    output: &Path,
    capacity: u32,
    rotate_z_180: bool,
) -> Result<()> {
    with_renderer(|assets, renderer| {
        let data = match request(assets, AssetCommand::Level(input.to_owned()))? {
            AssetReply::Level(data) => data,
            _ => return Err("asset worker returned an unexpected reply".into()),
        };
        render(
            renderer,
            RenderCommand::Level {
                data,
                input: input.to_owned(),
                output: output.to_owned(),
                capacity,
                rotate_z_180,
            },
        )
    })
}

/// Run a finite offscreen render loop. At most one GPU/PNG job is in flight; newer frame requests
/// are coalesced while the renderer is busy, so a slow renderer cannot build an unbounded queue.
pub(super) fn live_level(
    input: &Path,
    output: &Path,
    capacity: u32,
    rotate_z_180: bool,
    duration: Duration,
) -> Result<()> {
    with_renderer(|assets, renderer| {
        let data = match request(assets, AssetCommand::Level(input.to_owned()))? {
            AssetReply::Level(data) => data,
            _ => return Err("asset worker returned an unexpected reply".into()),
        };
        render(
            renderer,
            RenderCommand::PrepareLevel {
                data,
                input: input.to_owned(),
                capacity,
                rotate_z_180,
            },
        )?;
        let rate = Rate::new(60, 1)?;
        let fixed = FixedClock::with_rate(rate, Duration::from_millis(250), 4)?;
        let mut scheduler = Scheduler::new(fixed, rate);
        // This preview has no display refresh feedback; 60 Hz is an explicit target, not a detected monitor rate.
        let mut adaptive = AdaptivePacer::new(rate, 64)?;
        let start = Instant::now();
        let mut pending: Option<(Ticket<()>, Duration)> = None;
        let mut frames = 0u64;
        let mut busy = 0u64;
        let mut missed = 0u128;
        let mut dropped = Duration::ZERO;
        let mut next_report = Duration::from_secs(1);
        println!("Live offscreen preview: 60 Hz simulation, initial 60 Hz frame requests.");
        loop {
            let now = Instant::now()
                .checked_duration_since(start)
                .ok_or("monotonic clock moved backwards")?;
            if let Some((ticket, submitted)) = &pending {
                if ticket.try_receive()?.is_some() {
                    frames = frames.saturating_add(1);
                    let service = now.saturating_sub(*submitted);
                    if let Some(change) = adaptive.record(
                        now,
                        WorkSample {
                            pipeline: Some(service),
                            ..Default::default()
                        },
                    )? {
                        scheduler.set_frame_rate(change.rate, now)?;
                        println!(
                            "Offscreen workload adjusted target to {:.2} requests/s (span {}).",
                            change.rate.hz(),
                            change.span
                        );
                    }
                    pending = None;
                }
            }
            let plan = scheduler.poll(now)?;
            dropped = dropped.saturating_add(plan.simulation.dropped);
            missed = missed.saturating_add(plan.frame.missed);
            if plan.frame.due {
                if pending.is_none() {
                    let simulation_seconds = (plan.total_ticks as f64
                        + f64::from(plan.simulation.alpha))
                        * scheduler.tick_seconds();
                    let ticket = renderer
                        .try_submit(RenderCommand::LiveFrame {
                            output: output.to_owned(),
                            simulation_seconds,
                        })
                        .map_err(|error| error.to_string())?;
                    pending = Some((ticket, now));
                } else {
                    busy = busy.saturating_add(1);
                }
            }
            if now >= next_report {
                println!("{:.2}s: {} simulation ticks, {} completed offscreen frames, {} busy requests, {} missed deadlines",
                    now.as_secs_f64(), plan.total_ticks, frames, busy, missed);
                next_report = now.saturating_add(Duration::from_secs(1));
            }
            if now >= duration {
                println!(
                    "Stopping preview after {} ticks; simulation time discarded: {:?}",
                    plan.total_ticks, dropped
                );
                break;
            }
            let after_work = Instant::now()
                .checked_duration_since(start)
                .ok_or("monotonic clock moved backwards")?;
            let wake = plan.next_wake.unwrap_or(duration).min(duration);
            if let Some(wait) = wake.checked_sub(after_work) {
                if !wait.is_zero() {
                    thread::sleep(wait);
                }
            }
        }
        if let Some((ticket, _)) = pending {
            ticket.wait()?;
            frames = frames.saturating_add(1);
        }
        println!("Completed {frames} offscreen frames; {busy} requests coalesced while busy, {missed} missed deadlines.");
        Ok(())
    })
}
