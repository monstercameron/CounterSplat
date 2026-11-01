// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/frag/particleUpdaterRespawn.js
    if (outLife >= uniform.lifetime) {
        let subtractAmount = max(uniform.lifetime, uniform.numParticles * particleRate);
        outLife = outLife - subtractAmount;
        visMode = 1.0;
    }
    visMode = select(visMode, 1.0, outLife < 0.0);
