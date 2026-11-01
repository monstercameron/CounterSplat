// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/common/frag/fog.js
#include "fogMathPS"

var<private> dBlendModeFogFactor : f32 = 1.0;

#if (FOG != NONE)
    uniform fog_color : vec3f;
    
    #if (FOG == LINEAR)
        uniform fog_start : f32;
        uniform fog_end : f32;
    #else
        uniform fog_density : f32;
    #endif
#endif

#ifdef VERTEXSHADER
    fn getFogFactor(depth: f32) -> f32 {
#else
    fn getFogFactor() -> f32 {
        let depth = pcPosition.z / pcPosition.w;
#endif

    #if (FOG == LINEAR)
        return evaluateFogFactorLinear(depth, uniform.fog_start, uniform.fog_end);
    #elif (FOG == EXP)
        return evaluateFogFactorExp(depth, uniform.fog_density);
    #elif (FOG == EXP2)
        return evaluateFogFactorExp2(depth, uniform.fog_density);
    #else
        return 1.0;
    #endif
}

#ifdef VERTEXSHADER
    fn addFog(color: vec3f, depth: f32) -> vec3f {
        #if (FOG != NONE)
            return mix(uniform.fog_color * dBlendModeFogFactor, color, getFogFactor(depth));
        #else
            return color;
        #endif
    }
#else
    fn addFog(color: vec3f) -> vec3f {
        #if (FOG != NONE)
            return mix(uniform.fog_color * dBlendModeFogFactor, color, getFogFactor());
        #else
            return color;
        #endif
    }
#endif
