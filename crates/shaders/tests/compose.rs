use countersplat_shaders::preprocess::{preprocess, preprocess_source, Defines, Error};
use countersplat_shaders::{chunk_by_key, CHUNKS, KEYS};

#[test]
fn extracted_counts_match_the_engine_collection() {
    assert_eq!(
        CHUNKS.len(),
        267,
        "267 WGSL chunk files extracted from the engine"
    );
    assert_eq!(
        KEYS.len(),
        229,
        "229 include keys mapped from the four collection files"
    );
}

#[test]
fn every_mapped_key_resolves() {
    for (key, _) in KEYS {
        assert!(
            chunk_by_key(key).is_some(),
            "key {key} must resolve to a chunk"
        );
    }
}

/// Keys that cannot resolve from the engine's source files. The gsplat declarations are generated
/// at runtime by the engine (not present in any source file), and the rest are general engine
/// lighting/material includes that splat rendering does not use. The port must supply the first
/// group itself. Any change to this list fails the test on purpose.
const KNOWN_UNRESOLVED: &[&str] = &[
    "clusteredLightPS",
    "composePS",
    "gsplatCommonVS",
    "gsplatContainerDeclVS",
    "gsplatContainerReadVS",
    "gsplatVS",
    "lightDeclarationPS",
    "lightEvaluationPS",
    "lightFunctionLightPS",
    "litForwardMainPS",
    "litMainPS",
    "litMainVS",
    "litOtherMainPS",
    "litShadowMainPS",
];

#[test]
fn unresolved_keys_are_exactly_the_documented_gaps() {
    let mut unresolved: Vec<&str> = Vec::new();
    for (key, _) in KEYS {
        let mut d = Defines::new();
        if preprocess(key, &mut d).is_err() {
            unresolved.push(key);
        }
    }
    unresolved.sort();
    let mut expected = KNOWN_UNRESOLVED.to_vec();
    expected.sort();
    assert_eq!(unresolved, expected);
}

#[test]
fn ifdef_branches_follow_defines() {
    let src = "#ifdef FOO\nA\n#else\nB\n#endif\n";
    let mut d = Defines::new();
    assert_eq!(preprocess_source(src, &mut d).unwrap().trim(), "B");
    let mut d = Defines::new();
    d.insert("FOO".into(), String::new());
    assert_eq!(preprocess_source(src, &mut d).unwrap().trim(), "A");
}

#[test]
fn if_expressions_match_glsl_style_numbers_and_defined() {
    let src =
        "#if SH_BANDS > 1 && defined(PICK_PASS)\nX\n#elif SH_BANDS == 1\nY\n#else\nZ\n#endif\n";
    let mut d = Defines::new();
    d.insert("SH_BANDS".into(), "1".into());
    assert_eq!(preprocess_source(src, &mut d).unwrap().trim(), "Y");
    let mut d = Defines::new();
    d.insert("SH_BANDS".into(), "3".into());
    d.insert("PICK_PASS".into(), String::new());
    assert_eq!(preprocess_source(src, &mut d).unwrap().trim(), "X");
    let mut d = Defines::new();
    assert_eq!(preprocess_source(src, &mut d).unwrap().trim(), "Z");
}

#[test]
fn includes_expand_through_the_chunk_map() {
    let mut d = Defines::new();
    let out = preprocess("gsplatCenterVS", &mut d).unwrap();
    assert!(
        out.contains("matrix_model"),
        "centre shader should declare matrix_model"
    );
}

#[test]
fn unknown_directive_fails_loudly() {
    let mut d = Defines::new();
    assert!(matches!(
        preprocess_source("#pragma once\n", &mut d),
        Err(Error::UnknownDirective(_))
    ));
}

#[test]
fn unbalanced_conditional_is_an_error() {
    let mut d = Defines::new();
    assert_eq!(
        preprocess_source("#ifdef X\nA\n", &mut d),
        Err(Error::UnbalancedConditional)
    );
}

#[test]
fn sog_v2_sh_declarations_match_engine_golden_output() {
    use countersplat_shaders::declarations::{sog_streams, wgsl_input_declarations};
    let got = wgsl_input_declarations(&sog_streams(true, true));
    let want = include_str!("golden/sog_v2_sh_declarations.wgsl");
    assert_eq!(
        got.replace("\r\n", "\n").trim_end(),
        want.replace("\r\n", "\n").trim_end()
    );
}

#[test]
fn gsplat_vertex_path_composes_with_runtime_declarations() {
    use countersplat_shaders::declarations::{sog_streams, wgsl_input_declarations};
    use countersplat_shaders::preprocess::{preprocess_with, Overrides};
    let mut overrides = Overrides::new();
    overrides.insert(
        "gsplatContainerDeclarationsVS".to_string(),
        wgsl_input_declarations(&sog_streams(true, true)),
    );
    overrides.insert(
        "gsplatDeclarationsVS".to_string(),
        "#include \"gsplatContainerDeclVS\"".to_string(),
    );
    // The SOG resource's read code (gsplat-sog-resource.js: readWGSL).
    overrides.insert(
        "gsplatReadVS".to_string(),
        "#include \"gsplatSogVS\"".to_string(),
    );
    let mut d = Defines::new();
    let out = preprocess_with("gsplatVS", &mut d, &overrides).expect("gsplatVS should compose");
    assert!(
        out.contains("var means_l: texture_2d<f32>;"),
        "stream declarations must be inlined"
    );
}
