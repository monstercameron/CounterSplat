// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/linearizeDepth.js
#ifndef LINEARIZE_DEPTH
#define LINEARIZE_DEPTH

fn linearizeDepthWithParams(z: f32, cameraParams: vec4f) -> f32 {
    if (cameraParams.w == 0.0) {
        return (cameraParams.z * cameraParams.y) / (cameraParams.y + z * (cameraParams.z - cameraParams.y));
    } else {
        return cameraParams.z + z * (cameraParams.y - cameraParams.z);
    }
}

#ifndef CAMERAPLANES
    #define CAMERAPLANES
    uniform camera_params: vec4f;   // x: 1 / camera_far,      y: camera_far,     z: camera_near,        w: is_ortho
#endif

fn linearizeDepth(z: f32) -> f32 {
    return linearizeDepthWithParams(z, uniform.camera_params);
}
#endif // LINEARIZE_DEPTH
