struct Camera { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
};
@vertex fn vs_main(@location(0) position: vec3f, @location(1) color: vec4f) -> VertexOut {
    var out: VertexOut;
    out.position = camera.view_projection * vec4f(position, 1.0);
    out.color = color;
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4f {
    return in.color;
}
