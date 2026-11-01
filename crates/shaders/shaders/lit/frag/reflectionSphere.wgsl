// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/reflectionSphere.js
#ifndef VIEWMATRIX
    #define VIEWMATRIX
    uniform matrix_view: mat4x4f;
#endif

var texture_sphereMap: texture_2d<f32>;
var texture_sphereMapSampler: sampler;
uniform material_reflectivity: f32;

fn calcReflection(reflDir: vec3f, gloss: f32) -> vec3f {
    let viewRotationMatrix = mat3x3f(uniform.matrix_view[0].xyz, uniform.matrix_view[1].xyz, uniform.matrix_view[2].xyz);
    let reflDirV: vec3f = viewRotationMatrix * reflDir;

    let m: f32 = 2.0 * sqrt(dot(reflDirV.xy, reflDirV.xy) + (reflDirV.z + 1.0) * (reflDirV.z + 1.0));
    let sphereMapUv: vec2f = reflDirV.xy / m + 0.5;

    return {reflectionDecode}(textureSample(texture_sphereMap, texture_sphereMapSampler, sphereMapUv));
}

fn addReflection(reflDir: vec3f, gloss: f32) {
    dReflection = dReflection + vec4f(calcReflection(reflDir, gloss), uniform.material_reflectivity);
}
