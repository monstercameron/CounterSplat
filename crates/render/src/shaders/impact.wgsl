struct Camera {
    view_projection: mat4x4f,
    right: vec4f,
    up: vec4f,
};
@group(0) @binding(0) var<uniform> camera: Camera;

const CORNERS = array<vec2f, 6>(
    vec2f(-1.0,-1.0), vec2f(1.0,-1.0), vec2f(-1.0,1.0),
    vec2f(-1.0,1.0), vec2f(1.0,-1.0), vec2f(1.0,1.0)
);
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) opacity: f32,
    @location(2) @interpolate(flat) kind: f32,
    @location(3) @interpolate(flat) variation: f32,
};
@vertex fn vs_main(@builtin(vertex_index) vertex: u32,
    @location(0) center_radius: vec4f, @location(1) normal_kind: vec4f,
    @location(2) style: vec4f) -> VertexOut {
    let uv = CORNERS[vertex];
    var right = camera.right.xyz;
    var up = camera.up.xyz;
    if normal_kind.w < 0.5 {
        let n = normalize(normal_kind.xyz);
        var reference = vec3f(0.0, 1.0, 0.0);
        if abs(n.y) > 0.9 { reference = vec3f(1.0, 0.0, 0.0); }
        right = normalize(cross(reference, n));
        up = cross(n, right);
    }
    let world = center_radius.xyz + (right * uv.x + up * uv.y) * center_radius.w;
    var out: VertexOut;
    out.position = camera.view_projection * vec4f(world, 1.0);
    out.uv = uv; out.opacity = style.x; out.kind = normal_kind.w; out.variation = style.y;
    return out;
}
fn hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1,311.7))) * 43758.5453);
}
fn noise(p: vec2f) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let blend = f * f * (vec2f(3.0) - 2.0 * f);
    return mix(mix(hash(cell), hash(cell + vec2f(1.0,0.0)), blend.x),
        mix(hash(cell + vec2f(0.0,1.0)), hash(cell + vec2f(1.0,1.0)), blend.x), blend.y);
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let radius = length(in.uv);
    if in.kind < 0.5 {
        // A dark puncture with an irregular, translucent scorch/chip edge.
        let grain = hash(floor(in.uv * 18.0) + vec2f(in.variation, in.variation * 0.37));
        let jagged = radius + (grain - 0.5) * 0.08;
        let core = 1.0 - smoothstep(0.18, 0.38, jagged);
        let scorch = 1.0 - smoothstep(0.25, 0.98, jagged);
        let rim = smoothstep(0.16,0.28,jagged) * (1.0 - smoothstep(0.39,0.55,jagged));
        let color = mix(vec3f(0.018,0.016,0.014), vec3f(0.22,0.20,0.18), rim * 0.75);
        return vec4f(color, in.opacity * clamp(core * 0.9 + scorch * 0.42, 0.0, 0.98));
    }
    // Soft, overlapping billboards form a short collision dust/smoke plume.
    let seed = vec2f(in.variation * 0.13, in.variation * 0.21);
    let broad = noise(in.uv * 3.2 + seed);
    let detail = noise(in.uv * 7.4 - seed * 1.7);
    let edge = radius + (broad - 0.5) * 0.28;
    let density = (1.0 - smoothstep(0.1, 1.0, edge)) * (0.35 + broad * 0.4 + detail * 0.25);
    return vec4f(0.22, 0.21, 0.20, in.opacity * density);
}
