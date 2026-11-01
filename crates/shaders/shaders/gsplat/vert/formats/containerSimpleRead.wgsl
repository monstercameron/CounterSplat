// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/gsplat/vert/formats/containerSimpleRead.js
    fn getCenter() -> vec3f { return loadDataCenter().xyz; }
    fn getColor() -> vec4f { return loadDataColor(); }
    fn getScale() -> vec3f { return vec3f(loadDataCenter().w); }
    fn getRotation() -> vec4f { return vec4f(0.0, 0.0, 0.0, 1.0); }
