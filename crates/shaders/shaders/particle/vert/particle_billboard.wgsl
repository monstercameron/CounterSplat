// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_billboard.js
    let rotationResult = rotateWithMatrix(quadXY, inAngle);
    let rotatedQuadXY = rotationResult.rotatedVec;
    rotMatrix = rotationResult.matrix;
    var localPos = billboard(particlePos, rotatedQuadXY);
