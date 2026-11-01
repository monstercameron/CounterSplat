// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/end.js
    var finalRgb: vec3f = combineColor(litArgs_albedo, litArgs_sheen_specularity, litArgs_clearcoat_specularity);

    finalRgb = finalRgb + litArgs_emission;
    finalRgb = addFog(finalRgb);
    finalRgb = toneMap(finalRgb);
    finalRgb = gammaCorrectOutput(finalRgb);
    output.color = vec4f(finalRgb, output.color.a);
