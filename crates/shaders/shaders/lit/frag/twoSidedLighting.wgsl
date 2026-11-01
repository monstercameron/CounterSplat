// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/twoSidedLighting.js
// Flips the normals of a back face to face the viewer, so that it is lit on the side that is seen.
// The vertex normal is the shading normal when no normal map is used, and it also drives the
// shadow normal offset and the directional lightmap. The TBN matrix is built from the unflipped
// normal before this runs, and only its normal column is flipped, leaving the tangent and binormal
// unchanged.
fn handleTwoSidedLighting() {
    if (!pcFrontFacing) {
        dVertexNormalW = -dVertexNormalW;

        #ifdef LIT_TBN
            dTBN[2] = -dTBN[2];
        #endif
    }
}
