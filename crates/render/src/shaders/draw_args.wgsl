// The instanced draw's arguments from the depth-key pass's visible count: one instance per 128 visible splats.

struct ArgsUniforms {
    vertices_per_instance: u32,
    splats_per_instance: u32,
};

@group(0) @binding(0) var<uniform> args_params: ArgsUniforms;
@group(0) @binding(1) var<storage, read> visible_count: u32;
// drawIndexedIndirect: indexCount, instanceCount, firstIndex, baseVertex, firstInstance.
@group(0) @binding(2) var<storage, read_write> draw_args: array<u32, 5>;

@compute @workgroup_size(1)
fn drawArgs() {
    draw_args[0] = args_params.vertices_per_instance;
    draw_args[1] = (visible_count + args_params.splats_per_instance - 1u) / args_params.splats_per_instance;
    draw_args[2] = 0u;
    draw_args[3] = 0u;
    draw_args[4] = 0u;
}
