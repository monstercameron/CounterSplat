// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/pass-other/litOtherMain.js
#ifdef PICK_PASS
    #include "pickPS"
#endif

#ifdef PREPASS_PASS
    #include "floatAsUintPS"
#endif

@fragment
fn fragmentMain(input: FragmentInput) -> FragmentOutput {

    #ifdef CLIP_PLANES
        applyClipPlanes(vPositionW);
    #endif

    #include "litUserMainStartPS"

    var output: FragmentOutput;
    
    evaluateFrontend();

    #ifdef PICK_PASS
        output.color = getPickOutput();
        #ifdef DEPTH_PICK_PASS
            output.color1 = getPickDepth();
        #endif
    #endif

    #ifdef PREPASS_PASS
        output.color = float2vec4(vLinearDepth);
    #endif

    #include "litUserMainEndPS"

    return output;
}
