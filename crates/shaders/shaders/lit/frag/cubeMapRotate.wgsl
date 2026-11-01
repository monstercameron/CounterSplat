// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/cubeMapRotate.js
#ifdef CUBEMAP_ROTATION
uniform cubeMapRotationMatrix: mat3x3f;
#endif

fn cubeMapRotate(refDir: vec3f) -> vec3f {
#ifdef CUBEMAP_ROTATION
    return refDir * uniform.cubeMapRotationMatrix;
#else
    return refDir;
#endif
}
