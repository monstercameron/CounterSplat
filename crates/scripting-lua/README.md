# CounterSplat Lua scripting

This crate embeds Lua 5.4 and exposes the ECS through an owned message
boundary. The Lua VM and ECS can each live on their own thread: scripts never
borrow a World or send Lua values between threads. ECS has no dependency on
Lua. See examples/threaded.rs for a runnable minimal integration.

## Host integration

Implement WorldBridge to forward a WorldOperation to the ECS owner and return
its WorldOutcome. This can wrap the engine's bounded worker client. The
included ChannelBridge is an alternative using a bounded standard-library
sync_channel of WorldRequest messages. The ECS owner receives requests, applies
their commands with World::apply_command, and sends the matching reply.

ScriptWorker::spawn creates the Lua VM on a dedicated OS thread. Its execute,
run_system, run_systems, systems and dispatch methods submit work and return
reply receivers; they report a full queue instead of buffering without bound.
ScriptOptions sets the pending-work capacity and the VM's memory and
instruction limits. Call shutdown after pending work completes.

The ECS owner must continue servicing requests while a script is running and
while the scripting worker shuts down. In particular, do not run a script while
holding a world lock or block the ECS worker waiting for that script's reply:
scripts make synchronous ECS requests. Optional ChannelBridge timeouts limit
the caller's wait; they do not cancel a command already received by the ECS.
An operation can therefore take effect even when the caller times out.

Each read and write is applied when the ECS worker processes that individual
request. A script is not a transaction: earlier writes remain if a later call
fails, and requests from other clients can interleave. ecs.snapshot is one
point-in-time read, but neither requests nor snapshots are tagged with a tick.
The engine scheduler must define tick ordering and coordinate workers if it
needs deterministic frame boundaries.

LuaRuntime provides the same VM without spawning a thread, for a host that
already owns a scripting thread. It is intentionally not Send. Only the bridge
is sent into ScriptWorker::spawn; LuaRuntime is constructed inside the worker.

## Lua API

Entity IDs are opaque, world-scoped strings. Retain them only while their world
exists and use ecs.is_alive before acting on an entity that may have been
despawned. Errors such as malformed IDs, stale writes, invalid component values
or disconnected workers are Lua errors; they propagate to the host unless the
script handles them.

- ecs.spawn() returns an entity ID.
- ecs.despawn(id, recursive) returns the number removed. recursive defaults to
  false, which detaches surviving children.
- ecs.is_alive(id), ecs.entities(), ecs.components(id), and ecs.query(names)
  inspect the world. An empty query matches all live entities.
- ecs.get(id, name), ecs.has(id, name), ecs.set(id, name, value), and
  ecs.remove(id, name) access dynamic components and registered Rust component
  adapters. get and remove return nil when the component is absent.
- ecs.get_many(id, names) reads several components into a name-keyed table;
  missing components are omitted and JSON null remains ecs.null.
- ecs.set_field(id, name, field, value) changes one field on an existing
  object-valued component without replacing its other fields. Dynamic objects
  can gain fields; registered adapters apply their usual validation. A missing,
  non-object, or invalid target raises an error.
- ecs.set_fields(id, name, fields) updates several fields together. Registered
  adapters control validation; the built-in scene adapters commit the combined
  update only when it is valid.
- ecs.get_resource(name), ecs.set_resource(name, value), and
  ecs.remove_resource(name) access dynamic resources.
- ecs.set_parent(child, parent_or_nil), ecs.parent(id), and ecs.children(id)
  manage the entity hierarchy.
- ecs.emit(name, value) queues an ECS event; ecs.drain(name) returns and
  consumes the queued values for that name.
- ecs.snapshot() returns a point-in-time table with entities and resources.
  Each entity entry has id, optional parent, and components fields. Only
  script-visible components and dynamic resources are included.
- ecs.system(name, function(dt) ... end) registers or replaces a named system.
  ecs.remove_system(name) removes it. The host runs one system or the current
  list; run_systems visits the names present at the start in registration
  order, skipping names removed before their turn. A failure stops the pass.
- ecs.on(name, function(payload) ... end) registers an additional handler for a
  host-dispatched event; ecs.off(name) removes all handlers for that name.
  Dispatch takes a copy of the handler list and invokes it in registration
  order. These host events are distinct from the ECS event queues.
- engine.scene.push(id), engine.scene.pop(), and engine.scene.replace(id) emit
  requests on the `engine.scene` ECS event queue. The host validates scene IDs
  against its manifest and performs supported transitions after script execution.

For example:

    local player = ecs.spawn()
    ecs.set(player, "Position", {x = 0, y = 0})
    ecs.system("move", function(dt)
        local position = ecs.get(player, "Position")
        position.x = position.x + dt
        ecs.set(player, "Position", position)
    end)

Component, resource, event, snapshot, and host-event data are JSON values.
Reading a value makes a copy: change it and call set to write it back. Lua nil
represents a missing result; ecs.null represents JSON null. Use ecs.array()
when creating an empty JSON array (an ordinary empty Lua table is ambiguous).
Functions, cyclic tables, non-finite numbers, tables nested over 128 levels,
and values exceeding 100,000 nodes are rejected. Repeated references count for
each occurrence because JSON copies them. Avoid mixed array/map tables; JSON
object keys must be strings.
Rust components are script-visible only when explicitly registered with an ECS
adapter. Opaque native resources stay private.

## Runtime and platform

The host opens Lua's base, table, string, UTF-8 and math libraries. OS, IO,
package, debug and coroutine libraries are not opened, and filesystem loading
functions are removed. The default allocator limit is 64 MiB and the default
approximate instruction budget is 10 million per host call. Limits do not
bound time spent in Rust callbacks or waiting for the ECS, and are not a
complete security sandbox for hostile code. Host applications should choose
limits and isolate untrusted content to suit their environment.

This native crate builds Lua 5.4 from vendored C sources via mlua. A working C
compiler and platform toolchain are required. The browser host uses a separate
Wasmoon Lua 5.4 backend with the same ECS operation and scene-request contract;
it does not compile this native crate for WASM. Browser scene scope and current
transition support are documented in `docs/scenes.md`.

## Per-entity physics and exports

`engine.physics.move(entity, delta)` sends an entity displacement through the host's collision owner when
available; `engine.physics.step(dt, skip_entity)` integrates enabled bodies and can optionally skip one
entity while its collision data loads. Use `engine.export(entity, name, spec)` to
make a numeric or boolean ECS component field or resource field editable in the entity inspector. Numeric
exports specify `min` and `max`. The same API works with the native and browser Lua runtimes; the host must
supply physics integration. See [entity physics and Lua tuning](../../docs/entity-physics.md) for examples,
events and current browser collision limits.

## Ground queries

`engine.physics.probe_ground(entity, position, radius, max_drop, token)` queues a generic
asynchronous floor query. The browser host dispatches `physics.ground_result` with the entity,
token, and a numeric `height` or `ecs.null` if no floor was found. Scripts choose how to react.
`engine.physics.motion_version(entity)` changes when `engine.physics.move` applies movement;
capture it when submitting a query and compare it on receipt to reject stale results. A host
must implement the corresponding query and event delivery for the result to arrive.
