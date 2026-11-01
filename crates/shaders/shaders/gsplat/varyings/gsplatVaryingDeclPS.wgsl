// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/varyings/gsplatVaryingDeclPS.js
varying @interpolate(flat, either) user_{name}: {type};
fn get{funcName}() -> {type} { return user_{name}; }
