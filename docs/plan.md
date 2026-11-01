# Port plan and status

1. Map source responsibilities and identify game-specific coupling.
2. Establish a one-way workspace dependency graph for core, assets, shaders, rendering, physics, and hosts.
3. Port the reusable scan, rendering, timing, and collision code through explicit public APIs.
4. Add a small host, local visual preview, source notices, and focused architecture documentation.
5. Verify crate boundaries, native tests, GPU rendering/streaming, and WebAssembly library compilation.

The initial extraction implements these steps. Follow-on work can add a unified interactive host, additional physics
shapes and streamed colliders, and profiled indirect rendering behind independently tested interfaces.

## Follow-on architecture

The standalone ECS, Lua bridge, and native worker supervisor have been added. The viewer now runs asset loading and
rendering on different workers; a separate integration example connects ECS, Lua, and physics on their own workers.
The browser host now presents directly to a WebGPU canvas and uses a Web Worker for asset decoding. A single host
combining browser rendering with ECS, scripting, and physics remains a follow-on integration step.
