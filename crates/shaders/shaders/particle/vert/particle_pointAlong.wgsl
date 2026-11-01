// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_pointAlong.js
    // not the fastest way, but easier to plug in; TODO: create rot matrix right from vectors
    inAngle = atan2(velocityV.x, velocityV.y);
