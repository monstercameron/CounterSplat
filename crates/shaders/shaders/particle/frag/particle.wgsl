// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particle.js
varying texCoordsAlphaLife: vec4f;

var colorMap: texture_2d<f32>;
var colorMapSampler: sampler;
var colorParam: texture_2d<f32>;
var colorParamSampler: sampler;

uniform graphSampleSize: f32;
uniform graphNumSamples: f32;

#ifndef CAMERAPLANES
    #define CAMERAPLANES
    uniform camera_params: vec4f;
#endif

#ifdef SOFT
    uniform softening: f32;
#endif
uniform colorMult: f32;

@fragment
fn fragmentMain(input: FragmentInput) -> FragmentOutput {
    var output: FragmentOutput;

    let tex: vec4f  = textureSample(colorMap, colorMapSampler, vec2f(input.texCoordsAlphaLife.x, 1.0 - input.texCoordsAlphaLife.y));
    var ramp: vec4f = textureSample(colorParam, colorParamSampler, vec2f(input.texCoordsAlphaLife.w, 0.0));
    ramp = vec4f(ramp.rgb * uniform.colorMult, ramp.a);

    ramp.a = ramp.a + input.texCoordsAlphaLife.z;

    var rgb: vec3f = tex.rgb * ramp.rgb;
    var a: f32 = tex.a * ramp.a;
