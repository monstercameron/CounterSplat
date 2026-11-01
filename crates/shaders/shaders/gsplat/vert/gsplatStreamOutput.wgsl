// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/vert/gsplatStreamOutput.js
fn write{funcName}(value: {returnType}) {
#if {defineGuard}
    processOutput.{colorSlot} = value;
#endif
}
