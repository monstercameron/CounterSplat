# Threaded subsystem example

This native example runs actual ECS, Lua and Rapier physics state on three different owner threads. Lua creates an
entity and its initial position. The host reads that position from ECS, asks physics to simulate a falling box,
publishes the resulting position back to ECS, and asks a Lua system to read it. The output includes the thread IDs
and the final position.

```powershell
cargo run -p countersplat-threaded-demo
cargo run -p countersplat-threaded-demo -- --realtime 3
cargo test -p countersplat-threaded-demo
```

The ECS and physics owners use bounded runtime worker queues. The Lua VM is built on its own scripting thread. The
Lua bridge sends owned operations and replies to the ECS owner; the host must keep ECS running while it waits for
Lua, and shuts Lua down before ECS to prevent a request cycle. A queue-full error is reported to the caller, and a
reply timeout does not cancel an already accepted operation.

The default command is a short native round trip. `--realtime [seconds]` runs an actual 60 Hz fixed-step clock,
advances the physics worker, publishes the pose to ECS, and runs a Lua system for each tick. It counts software frame
requests but does not render them. Asset and render integration belong to the separate viewer host; browser workers
need a separate backend. See [timing](../../docs/timing.md) for the scheduler and its limits.
