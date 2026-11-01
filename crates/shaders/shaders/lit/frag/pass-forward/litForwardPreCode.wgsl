// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/pass-forward/litForwardPreCode.js
#include "basePS"
#include "sphericalPS"
#include "decodePS"
#include "gammaPS"
#include "tonemappingPS"
#include "fogPS"

// 9-slice support code
#if LIT_NONE_SLICE_MODE == SLICED
    #include "baseNineSlicedPS"
#elif LIT_NONE_SLICE_MODE == TILED
    #include "baseNineSlicedTiledPS"
#endif

// flat shading
#ifdef FLAT_SHADING
    #include "flatNormalPS"
#endif

// TBN
#ifdef LIT_TBN
    #include "TBNPS"
#endif

// two sided lighting
#if defined(LIT_NEEDS_NORMAL) && defined(LIT_TWO_SIDED_LIGHTING)
    #include "twoSidedLightingPS"
#endif

