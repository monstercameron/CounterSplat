// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/gamma.js
#include "decodePS"

#if (GAMMA == SRGB)

    fn gammaCorrectInput(color: f32) -> f32 {
        return decodeGammaFloat(color);
    }

    fn gammaCorrectInputVec3(color: vec3f) -> vec3f {
        return decodeGamma3(color);
    }

    fn gammaCorrectInputVec4(color: vec4f) -> vec4f {
        return vec4f(decodeGamma3(color.xyz), color.w);
    }

    fn gammaCorrectOutput(color: vec3f) -> vec3f {
        return pow(color + 0.0000001, vec3f(1.0 / 2.2));
    }

#else // NONE

    fn gammaCorrectInput(color: f32) -> f32 {
        return color;
    }

    fn gammaCorrectInputVec3(color: vec3f) -> vec3f {
        return color;
    }

    fn gammaCorrectInputVec4(color: vec4f) -> vec4f {
        return color;
    }

    fn gammaCorrectOutput(color: vec3f) -> vec3f {
        return color;
    }

#endif
