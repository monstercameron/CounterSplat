// Environment background: a fullscreen triangle that samples the cubemap along each pixel's view ray.
// The ray is built from the inverse projection and the view rotation (the view's transpose), so the sky
// does not move with camera translation. Display and HDR targets use separate fragment entry points.

struct SkyUniforms {
    inverse_projection: mat4x4f,
    view: mat4x4f,
    exposure: f32,
    rotation_sin: f32,
    rotation_cos: f32,
};

@group(0) @binding(0) var<uniform> sky: SkyUniforms;
@group(0) @binding(1) var environment: texture_cube<f32>;
@group(0) @binding(2) var environment_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) ndc: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    // Covers the screen with one oversized triangle: (0,0), (2,0), (0,2) in UV space.
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: VertexOutput;
    out.position = vec4f(ndc, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

// Narkowicz's fit of the ACES filmic curve.
fn tone_map(x: vec3f) -> vec3f {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3f(0.0), vec3f(1.0));
}

fn sky_radiance(ndc: vec2f) -> vec3f {
    let clip = sky.inverse_projection * vec4f(ndc, 1.0, 1.0);
    let view_dir = clip.xyz / clip.w;
    // World direction = transpose(view rotation) × view_dir; the columns of `view` give the rows of the transpose.
    let world_dir = vec3f(dot(sky.view[0].xyz, view_dir), dot(sky.view[1].xyz, view_dir), dot(sky.view[2].xyz, view_dir));
    let rotated_dir = vec3f(
        sky.rotation_cos * world_dir.x - sky.rotation_sin * world_dir.z,
        world_dir.y,
        sky.rotation_sin * world_dir.x + sky.rotation_cos * world_dir.z
    );
    return textureSample(environment, environment_sampler, normalize(rotated_dir)).rgb * sky.exposure;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return vec4f(tone_map(sky_radiance(in.ndc)), 1.0);
}

@fragment
fn fs_linear_hdr(in: VertexOutput) -> @location(0) vec4f {
    // The half-float target must not overflow to infinity at high exposures.
    return vec4f(min(sky_radiance(in.ndc), vec3f(65504.0)), 1.0);
}
