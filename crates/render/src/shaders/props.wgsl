// Solid props (boxes and upright cylinders) lit by one directional light. Each instance places a unit mesh with a
// position, rotation quaternion and per-axis scale; cylinder caps take the instance's cap colour.
struct Camera {
    view_projection: mat4x4<f32>,
    // xyz: direction towards the light; w: exposure multiplier.
    light: vec4f,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec3f,
    @location(1) normal: vec3f,
    @location(2) cap: f32,
    @location(3) instance_position: vec3f,
    @location(4) instance_rotation: vec4f,
    @location(5) instance_scale: vec3f,
    @location(6) instance_color: vec3f,
    @location(7) instance_cap_color: vec3f,
};

struct VertexOut {
    @builtin(position) clip: vec4f,
    @location(0) normal: vec3f,
    @location(1) color: vec3f,
};

fn rotate(q: vec4f, v: vec3f) -> vec3f {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

@vertex fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = in.instance_position + rotate(in.instance_rotation, in.position * in.instance_scale);
    out.clip = camera.view_projection * vec4f(world, 1.0);
    // Scaling a normal by the inverse of the (axis-aligned, local) scale keeps it perpendicular to the surface.
    out.normal = rotate(in.instance_rotation, in.normal / in.instance_scale);
    out.color = mix(in.instance_color, in.instance_cap_color, in.cap);
    return out;
}

@fragment fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let n = normalize(in.normal);
    let l = normalize(camera.light.xyz);
    // Wrapped diffuse keeps the shaded side readable under the barn's soft interior light.
    let diffuse = max(dot(n, l) * 0.6 + 0.4, 0.0);
    let lit = in.color * (0.25 + 0.85 * diffuse) * camera.light.w;
    return vec4f(lit, 1.0);
}
