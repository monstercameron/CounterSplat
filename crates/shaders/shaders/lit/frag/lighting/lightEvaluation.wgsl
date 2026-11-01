// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/lighting/lightEvaluation.js
#if defined(LIGHT{i})
    evaluateLight{i}(
        #if defined(LIT_IRIDESCENCE)
            iridescenceFresnel
        #endif
    );
#endif
