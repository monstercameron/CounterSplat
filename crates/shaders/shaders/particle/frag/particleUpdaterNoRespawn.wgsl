// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particleUpdaterNoRespawn.js
    if (outLife >= uniform.lifetime) {
        outLife = outLife - max(uniform.lifetime, uniform.numParticles * particleRate);
        visMode = -1.0;
    }
