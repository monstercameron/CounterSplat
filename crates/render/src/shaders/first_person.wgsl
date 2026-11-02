// A chroma-keyed frame is composited into the linear HDR scene before grading.
// Neural-Sight's firing-frame alpha contains a weapon-protection mask, not image
// transparency, so coverage comes from the green key instead of source alpha.
struct Settings { key: vec4f, view: vec4f, motion: vec4f, blur: vec4f };
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var<uniform> settings: Settings;
@group(0) @binding(2) var previous_image: texture_2d<f32>;
struct VertexOut { @builtin(position) position: vec4f, @location(0) uv: vec2f };
@vertex fn vertex(@builtin(vertex_index) i: u32) -> VertexOut {
    let p = array<vec2f, 3>(vec2f(-1., -1.), vec2f(3., -1.), vec2f(-1., 3.));
    var out: VertexOut;
    out.position = vec4f(p[i], 0., 1.);
    out.uv = vec2f(p[i].x * .5 + .5, .5 - p[i].y * .5);
    return out;
}
fn linear(c: vec3f) -> vec3f {
    return select(c / 12.92, pow((c + .055) / 1.055, vec3f(2.4)), c > vec3f(.04045));
}
fn keyed(frame: texture_2d<f32>, pixel: vec2i, dimensions: vec2i) -> vec4f {
    let c = textureLoad(frame, clamp(pixel, vec2i(0), dimensions - vec2i(1)), 0).rgb;
    let green = c.g - max(c.r, c.b);
    let alpha = (1. - smoothstep(settings.key.x, settings.key.x + settings.key.y, green)) * settings.key.w;
    let clean = vec3f(c.r, min(c.g, max(c.r, c.b) + settings.key.z), c.b);
    return vec4f(linear(clean) * alpha, alpha);
}
fn sample_keyed(frame: texture_2d<f32>, uv: vec2f, dim: vec2i) -> vec4f {
    let pos = uv * vec2f(dim) - vec2f(.5);
    let lo = vec2i(floor(pos));
    let t = fract(pos);
    return mix(mix(keyed(frame,lo,dim), keyed(frame,lo+vec2i(1,0),dim), t.x),
               mix(keyed(frame,lo+vec2i(0,1),dim), keyed(frame,lo+vec2i(1,1),dim), t.x), t.y);
}
// Blur premultiplied, keyed colors so the green screen cannot bleed into the
// weapon silhouette. Keep clip transitions inside the same sample path.
fn presented(uv: vec2f, dim: vec2i) -> vec4f {
    if any(uv < vec2f(0.)) || any(uv > vec2f(1.)) { return vec4f(0.); }
    let current = sample_keyed(image, uv, dim);
    if settings.motion.w >= .9999 { return current; }
    return mix(sample_keyed(previous_image, uv, dim), current, settings.motion.w);
}
// Trail taps use one keyed source texel each; the center retains the full
// bilinear reconstruction. This bounds turn-time cost while keeping the green
// key and clip blending in premultiplied space.
fn presented_trail(uv: vec2f, dim: vec2i) -> vec4f {
    if any(uv < vec2f(0.)) || any(uv > vec2f(1.)) { return vec4f(0.); }
    let pixel = vec2i(floor(uv * vec2f(dim)));
    let current = keyed(image, pixel, dim);
    if settings.motion.w >= .9999 { return current; }
    return mix(keyed(previous_image, pixel, dim), current, settings.motion.w);
}
@fragment fn fragment(in: VertexOut) -> @location(0) vec4f {
    var uv = in.uv;
    // Preserve the footage aspect ratio and anchor its bottom to the scene.
    // Centering vertically on a taller scene leaves a visible hard cutoff at
    // the bottom of the viewmodel when the output is scaled to the viewport.
    let ratio = settings.view.y / max(settings.view.z, 1.);
    let source = settings.view.w;
    if ratio > source { uv.x = (uv.x - .5) * ratio / source + .5; }
    else { uv.y = (uv.y - 1.) * source / ratio + 1.; }
    // Lua supplies visual-only movement. Rotate around the bottom anchor so
    // the filmed arms remain attached to the bottom of the viewport.
    var p = uv - vec2f(.5, 1.) - settings.motion.xy;
    let c = cos(settings.motion.z); let s = sin(settings.motion.z);
    p = vec2f(c*p.x + s*p.y, -s*p.x + c*p.y);
    uv = p + vec2f(.5, 1.);
    uv.y -= settings.view.x;
    let dim = vec2i(textureDimensions(image));
    let center = presented(uv, dim);
    let span = settings.blur.xy;
    let pixel_span = length(span * vec2f(dim));
    // A subpixel span is already covered by the keyed bilinear sample. Use
    // fewer taps for short trails and reserve the wider filter for fast turns.
    if pixel_span < .45 { return center; }
    if pixel_span < 2.0 {
        return center * .5 + (presented_trail(uv - span, dim) + presented_trail(uv + span, dim)) * .25;
    }
    // A symmetric, weighted five-tap shutter keeps the sight centered and
    // softens the trail instead of producing a pair of hard ghost images.
    return center * .4
        + (presented_trail(uv - span * .5, dim) + presented_trail(uv + span * .5, dim)) * .2
        + (presented_trail(uv - span, dim) + presented_trail(uv + span, dim)) * .1;
}
