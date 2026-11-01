// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/internal/frag/immediateLine.js
    #include "gammaPS"
    varying color: vec4f;
    @fragment
    fn fragmentMain(input : FragmentInput) -> FragmentOutput {
        var output: FragmentOutput;
        output.color = vec4f(gammaCorrectOutput(decodeGamma3(input.color.rgb)), input.color.a);
        return output;
    }
