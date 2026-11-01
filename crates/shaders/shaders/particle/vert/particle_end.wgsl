// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_end.js
    localPos = localPos * scale * uniform.emitterScale;

    #ifdef SCREEN_SPACE
        // clip space spans the viewport width in x and its height in y, so scale the offset x by
        // height / width to keep the particle square, sized relative to the viewport height
        localPos.x *= uniform.viewport_size.y * uniform.viewport_size.z;
    #endif

    localPos = localPos + particlePos;

    #ifdef SCREEN_SPACE
        output.position = vec4f(localPos.x, localPos.y, 0.0, 1.0);
    #else
        output.position = uniform.matrix_viewProjection * vec4f(localPos.xyz, 1.0);
    #endif
