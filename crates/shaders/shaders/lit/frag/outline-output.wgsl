// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/outline-output.js
#ifdef PCOUTLINE_PASS
output.color = vec4f(gammaCorrectOutput(uniform.pcOutlineColor), output.color.a);
#endif
