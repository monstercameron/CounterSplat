struct Camera { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) opacity: f32,
};
@vertex fn vs_main(@location(0) vertex: vec4f) -> VertexOut {
    var out: VertexOut;
    out.position = camera.view_projection * vec4f(vertex.xyz, 1.0);
    out.opacity = vertex.w;
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4f {
    return vec4f(3.2, 1.55, 0.35, in.opacity * 0.9);
}
