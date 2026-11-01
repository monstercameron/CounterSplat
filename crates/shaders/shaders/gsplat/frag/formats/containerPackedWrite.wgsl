// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/frag/formats/containerPackedWrite.js
fn writeSplat(center: vec3f, rotation: vec4f, scale: vec3f, color: vec4f) {
    writeDataColor(color);
    #ifndef GSPLAT_COLOR_ONLY
        writeDataTransformA(vec4u(bitcast<u32>(center.x), bitcast<u32>(center.y), bitcast<u32>(center.z), pack2x16float(rotation.xy)));
        writeDataTransformB(vec4u(pack2x16float(vec2f(rotation.z, scale.x)), pack2x16float(scale.yz), 0u, 0u));
    #endif
}
