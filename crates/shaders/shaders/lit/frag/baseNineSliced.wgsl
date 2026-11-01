// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/baseNineSliced.js
#define NINESLICED

varying vMask: vec2f;
varying vTiledUv: vec2f;

uniform innerOffset: vec4f;
uniform outerScale: vec2f;
uniform atlasRect: vec4f;

var<private> nineSlicedUv: vec2f;
