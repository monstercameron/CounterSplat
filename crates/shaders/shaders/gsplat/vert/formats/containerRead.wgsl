// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/vert/formats/containerRead.js
fn getCenter() -> vec3f {
    #include "gsplatContainerUserReadVS"
    return splatCenter;
}

fn getRotation() -> vec4f {
    return splatRotation;
}

fn getScale() -> vec3f {
    return splatScale;
}

fn getColor() -> vec4f {
    return splatColor;
}
