// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/particle/vert/particle_mesh.js
var localPos = meshLocalPos;
let rotResultXY = rotateWithMatrix(localPos.xy, inAngle);
localPos = vec3f(rotResultXY.rotatedVec, localPos.z);
rotMatrix = rotResultXY.matrix;

let rotResultYZ = rotateWithMatrix(localPos.yz, inAngle);
localPos = vec3f(localPos.x, rotResultYZ.rotatedVec);
rotMatrix = rotResultYZ.matrix;

billboard(particlePos, quadXY);
