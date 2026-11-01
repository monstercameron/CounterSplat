// Linear HDR scene -> half-resolution bloom -> color correction -> sRGB surface.
// The UI is composited separately after this pass.
struct Grade {
    exposure_contrast_saturation_temperature: vec4f,
    tint_shadows_highlights_vignette: vec4f,
    bloom_threshold_grain_enabled: vec4f,
    filmic_reserved: vec4f,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var<uniform> grade: Grade;
@group(0) @binding(3) var bloom: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};
@vertex fn fullscreen(@builtin(vertex_index) index: u32) -> VertexOut {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn luminance(c: vec3f) -> f32 { return dot(c, vec3f(0.2126, 0.7152, 0.0722)); }

// Average neighboring HDR pixels before soft-knee extraction to avoid fireflies.
@fragment fn prefilter(in: VertexOut) -> @location(0) vec4f {
    let texel = 0.5 / vec2f(textureDimensions(source));
    let average = (textureSample(source, linear_sampler, in.uv + vec2f(-texel.x, -texel.y)).rgb
                 + textureSample(source, linear_sampler, in.uv + vec2f( texel.x, -texel.y)).rgb
                 + textureSample(source, linear_sampler, in.uv + vec2f(-texel.x,  texel.y)).rgb
                 + textureSample(source, linear_sampler, in.uv + vec2f( texel.x,  texel.y)).rgb) * 0.25;
    // Exposure precedes highlight extraction so bloom follows the image brightness.
    // Cap the half-float target to avoid infinities at extreme HDR/exposure values.
    let c = min(average * exp2(grade.exposure_contrast_saturation_temperature.x), vec3f(60000.0));
    let brightness = max(max(c.r, c.g), c.b);
    let threshold = grade.bloom_threshold_grain_enabled.y;
    let knee = max(threshold * 0.5, 0.0001);
    let soft = clamp(brightness - threshold + knee, 0.0, 2.0 * knee);
    let contribution = max(brightness - threshold, soft * soft / (4.0 * knee)) / max(brightness, 0.0001);
    return vec4f(c * contribution, 1.0);
}

// Five bilinear taps implement a separable, normalized nine-tap Gaussian.
fn blur(uv: vec2f, step: vec2f) -> vec3f {
    return textureSample(source, linear_sampler, uv).rgb * 0.227027
         + (textureSample(source, linear_sampler, uv + step * 1.384615).rgb
          + textureSample(source, linear_sampler, uv - step * 1.384615).rgb) * 0.316216
         + (textureSample(source, linear_sampler, uv + step * 3.230769).rgb
          + textureSample(source, linear_sampler, uv - step * 3.230769).rgb) * 0.070270;
}
@fragment fn blur_horizontal(in: VertexOut) -> @location(0) vec4f {
    return vec4f(blur(in.uv, vec2f(1.0 / f32(textureDimensions(source).x), 0.0)), 1.0);
}
@fragment fn blur_vertical(in: VertexOut) -> @location(0) vec4f {
    return vec4f(blur(in.uv, vec2f(0.0, 1.0 / f32(textureDimensions(source).y))), 1.0);
}

// An ACES-inspired fitted curve, not a full Academy color-management transform.
fn filmic(x: vec3f) -> vec3f {
    let v = max(x * 0.72, vec3f(0.0));
    return clamp((v * (2.51 * v + vec3f(0.03))) /
                 (v * (2.43 * v + vec3f(0.59)) + vec3f(0.14)), vec3f(0.0), vec3f(1.0));
}
fn noise(pixel: vec2f) -> f32 {
    return fract(sin(dot(pixel, vec2f(12.9898, 78.233))) * 43758.5453) - 0.5;
}

@fragment fn correct(in: VertexOut) -> @location(0) vec4f {
    let uv = (in.uv - vec2f(0.5)) * grade.filmic_reserved.yz + vec2f(0.5);
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) { return vec4f(0.0, 0.0, 0.0, 1.0); }
    var scene = textureSampleLevel(source, linear_sampler, uv, 0.0);
    if grade.bloom_threshold_grain_enabled.w < 0.5 { return scene; }
    // A bounded four-neighbor unsharp mask in linear HDR. Limit the detail
    // contribution so high-contrast highlights cannot create extreme halos.
    let sharpness = grade.filmic_reserved.w;
    if sharpness > 0.0 {
        let texel = 1.0 / vec2f(textureDimensions(source));
        let adjacent = (
            textureSampleLevel(source, linear_sampler, uv + vec2f(texel.x, 0.0), 0.0).rgb +
            textureSampleLevel(source, linear_sampler, uv - vec2f(texel.x, 0.0), 0.0).rgb +
            textureSampleLevel(source, linear_sampler, uv + vec2f(0.0, texel.y), 0.0).rgb +
            textureSampleLevel(source, linear_sampler, uv - vec2f(0.0, texel.y), 0.0).rgb
        ) * 0.25;
        let limit = max(scene.rgb, vec3f(0.04)) * 0.25;
        let detail = clamp(scene.rgb - adjacent, -limit, limit);
        scene = vec4f(clamp(scene.rgb + detail * sharpness, vec3f(0.0), vec3f(60000.0)), scene.a);
    }
    let p = grade.exposure_contrast_saturation_temperature;
    let q = grade.tint_shadows_highlights_vignette;
    var color = max(min(scene.rgb, vec3f(60000.0)) * exp2(p.x) + textureSampleLevel(bloom, linear_sampler, uv, 0.0).rgb * grade.bloom_threshold_grain_enabled.x, vec3f(0.0));
    color *= exp2(vec3f(p.w * 0.30, q.x * 0.15, -p.w * 0.30));
    let luma = luminance(color);
    let shadow_weight = 1.0 - smoothstep(0.08, 0.50, luma);
    let highlight_weight = smoothstep(0.30, 1.20, luma);
    color *= exp2((q.y * shadow_weight + q.z * highlight_weight) * 0.65);
    color = mix(vec3f(luminance(color)), color, p.z);
    color = vec3f(0.18) * pow(max(color, vec3f(0.0)) / 0.18, vec3f(p.y));
    if grade.filmic_reserved.x > 0.5 { color = filmic(color); }
    let edge = smoothstep(0.35, 0.75, length(uv - vec2f(0.5)));
    color *= 1.0 - q.w * edge;
    color += vec3f(noise(in.position.xy) * grade.bloom_threshold_grain_enabled.z);
    return vec4f(max(color, vec3f(0.0)), scene.a);
}
