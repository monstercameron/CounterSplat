struct Uniforms {
    view_projection: mat4x4<f32>,
    voxel: vec4<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

const CORNERS = array<vec3<f32>, 8>(
    vec3<f32>(-0.5,-0.5,-0.5), vec3<f32>(0.5,-0.5,-0.5),
    vec3<f32>(-0.5,0.5,-0.5), vec3<f32>(0.5,0.5,-0.5),
    vec3<f32>(-0.5,-0.5,0.5), vec3<f32>(0.5,-0.5,0.5),
    vec3<f32>(-0.5,0.5,0.5), vec3<f32>(0.5,0.5,0.5)
);
const EDGES = array<u32, 24>(0u,1u, 2u,3u, 4u,5u, 6u,7u,
    0u,2u, 1u,3u, 4u,6u, 5u,7u, 0u,4u, 1u,5u, 2u,6u, 3u,7u);
@vertex fn vs_main(@builtin(vertex_index) vertex: u32, @location(0) center: vec3<f32>) -> @builtin(position) vec4<f32> {
    let position = center + CORNERS[EDGES[vertex]] * uniforms.voxel.x;
    return uniforms.view_projection * vec4<f32>(position, 1.0);
}
@fragment fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.12, 0.78, 0.86, 0.62);
}
