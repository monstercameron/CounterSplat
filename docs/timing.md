# Simulation time and frame pacing

## Clock and host contract

`countersplat-core` does not read a wall clock or sleep. A host supplies elapsed monotonic timestamps from one origin
to `scheduler::Scheduler::poll`. The first poll establishes the origin and requests an initial frame. Later polls
return the number and sequence of fixed simulation ticks, interpolation fraction, time deliberately discarded,
software frame decision, and the earliest next tick or frame deadline. Run the returned simulation ticks in order
before consuming the frame decision.

`Rate` stores an exact rational events-per-second value. For example `Rate::new(60, 1)` and
`Rate::new(60_000, 1_001)` represent 60 and approximately 59.94 Hz without repeatedly accumulating a rounded
nanosecond interval. The fixed clock performs constant-time arithmetic even for very long stalls. Its configured
`max_frame` and `max_ticks` bound catch-up; extra whole ticks are discarded and reported, while the fractional
remainder is retained. `step_seconds` provides the fixed duration for physics. Floating-point simulation, external
inputs, and thread ordering still need their own determinism policies.

A timestamp moving backwards returns `NonMonotonicTime` without consuming scheduler time. Pausing and resuming clears
the fractional tick and discards the interval surrounding the transition; frame requests may continue while the
simulation is paused. Poll immediately before a transition if the preceding elapsed time matters. For a game that
must never discard simulation time, use a different host policy: this clock intentionally bounds catch-up.

```rust
use countersplat_core::{rate::Rate, scheduler::Scheduler, time::FixedClock};
use std::time::Duration;

let tick_rate = Rate::new(60, 1)?;
let fixed = FixedClock::with_rate(tick_rate, Duration::from_millis(250), 4)?;
let mut scheduler = Scheduler::new(fixed, Rate::new(60_000, 1_001)?);
let first = scheduler.poll(Duration::ZERO)?;
assert!(first.frame.due);
let next = scheduler.poll(Duration::from_millis(50))?;
assert_eq!(next.simulation.ticks, 3);
# Ok::<(), Box<dyn std::error::Error>>(())
```

The host must schedule another poll at or before `next_wake`; calling only when a frame completes couples simulation
to a slow renderer. For a native event loop, integrate the deadline with that loop's wake mechanism rather than
blocking the platform thread. A small headless host may sleep, then re-read its monotonic clock because sleeps can
overshoot. The threaded example does this while ECS, Lua, and physics remain on their own workers.

## Frame requests and adaptive pacing

`pacing::DeadlinePacer` anchors rational deadlines to a phase rather than adding a rounded interval after every frame.
If the host arrives late, it issues at most one request, counts skipped deadlines, and resumes on the original phase.
A target rate change through `Scheduler::set_frame_rate` rebases the next deadline without emitting an extra frame.
These are **request** times; completion and actual presentation must be measured by the renderer/platform.

`pacing::AdaptivePacer` is an optional policy for a known fixed display rate. It accepts actual CPU, GPU, or single
in-flight pipeline durations. It uses the maximum available measurement, a bounded 60-sample 90th percentile, 10%
headroom, and a sustained-overload delay before reducing the frame target. It can move directly to the estimated
refresh span under sustained severe overload; recovery happens one refresh span at a time after two seconds of
headroom. It clears stale measurement history after a long gap. `max_span` is a host-chosen limit; beyond it the host
must still coalesce or drop requests when busy.

Only submit a GPU duration when actual GPU timestamp queries have measured GPU execution. A `pipeline` sample is
explicitly end-to-end service time for one in-flight request and can include GPU readback, image encoding, and
polling latency; it is not a GPU timestamp. An application that does not have measurements can use a fixed target.
The older callback-driven `FramePacer` is intended for a host with a callback per refresh, not as a substitute for a
native monotonic deadline or real presentation feedback.

## Run the examples

```powershell
cargo run -p countersplat-threaded-demo -- --realtime 3
cargo run -p countersplat-viewer -- live-level "path\to\level" ".artifacts\frame.png" 10 300000 z180
python scripts/preview.py --level "path\to\level"
```

The first command advances a real Rapier body and Lua system on fixed ticks and counts frame requests; it does not
draw. The live viewer loads a scene once, runs a 60 Hz simulation timeline and a small camera animation, and sends
frames to the rendering owner. Only one offscreen job may be in flight. Busy requests are coalesced, and the preview
uses the measured readback-and-PNG service time to adjust its request target. The browser canvas is a separate host:
it renders the streamed scene directly through WebGPU, advances the scheduler from animation callbacks, and decodes
chunks in a Web Worker. It requests frames on the browser callback cadence unless the optional FPS cap is enabled.
Its FPS and frame-time counters measure submitted-frame intervals, not GPU execution or display presentation.
The canvas host does not yet run the native Lua/physics example.

## Presentation limits

A software pacer cannot guarantee visible frame timing: the operating system may wake a sleeping thread late, and
GPU queues, compositor scheduling, refresh rate changes, and presentation mode affect when a frame appears. When a
using a windowed host, use actual presentation feedback where available and a supported present mode such as FIFO;
measure CPU, GPU, and displayed frame times separately. Adaptive behavior for variable-refresh displays and background
window policies remains host work. No claim of universally stutter-free presentation is made by these examples.

- [Rust monotonic `Instant`](https://doc.rust-lang.org/std/time/struct.Instant.html)
- [Rust `thread::sleep` scheduling](https://doc.rust-lang.org/std/thread/fn.sleep.html)
- [wgpu presentation modes](https://docs.rs/wgpu/30.0.1/wgpu/enum.PresentMode.html)

## Presentation and FPS caps

The browser host requests a frame on each `requestAnimationFrame` callback when its **Limit FPS** option is off.
The browser chooses the callback and canvas presentation cadence; the host no longer assumes a 60 Hz monitor.
The optional browser FPS cap controls software frame requests, but the browser still controls synchronization and
can reduce the achieved rate. A browser canvas cannot expose a true VSync-off switch.

For genuine native surface selection, use `countersplat-viewer window-level` (see the workspace README). VSync on
selects FIFO. VSync off selects Immediate only if the surface advertises it, and otherwise reports an error; it
does not silently label FIFO or mailbox as VSync off. The native off mode can run uncapped or with a cap, including
the current window monitor's reported refresh rate. The monitor cap updates when the window moves to another
monitor. `FrameLimiter` uses monotonic timestamps and rounds the exact period upward, avoiding bursts after a late
frame. Frame submission counts do not prove when the operating system actually presented a frame. Windowed
compositors and drivers can still influence what reaches the display.

The native window is a standalone viewer host, with GPU work on its window/event-loop thread and loading on the
asset worker; it is not yet the unified multi-worker game host.
