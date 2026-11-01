// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/vert/uv0.js
#ifdef NINESLICED
    fn getUv0() -> vec2f {
        var uv = vertex_position.xz;

        // offset inner vertices inside
        let positiveUnitOffset = clamp(vertex_position.xz, vec2f(0.0, 0.0), vec2f(1.0, 1.0));
        let negativeUnitOffset = clamp(-vertex_position.xz, vec2f(0.0, 0.0), vec2f(1.0, 1.0));

        uv = uv + ((-positiveUnitOffset * uniform.innerOffset.xy) + (negativeUnitOffset * uniform.innerOffset.zw)) * vertex_texCoord0.xy;

        uv = uv * -0.5 + vec2f(0.5, 0.5);
        uv = uv * uniform.atlasRect.zw + uniform.atlasRect.xy;

        dMaskGlobal = vertex_texCoord0.xy;

        return uv;
    }
#else
    fn getUv0() -> vec2f {
        return vertex_texCoord0;
    }
#endif
