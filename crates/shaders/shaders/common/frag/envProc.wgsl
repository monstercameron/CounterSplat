// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/envProc.js
#ifdef LIT_SKYBOX_INTENSITY
    uniform skyboxIntensity : f32;
#endif

fn processEnvironment(color : vec3f) -> vec3f {
    #ifdef LIT_SKYBOX_INTENSITY
        return color * uniform.skyboxIntensity;
    #else
        return color;
    #endif
}
