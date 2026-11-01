//! naga validation of the composed gsplat shaders for the SOG V2 SH format.
use countersplat_shaders::pipeline::{compose_gsplat, Stage};

fn validate(stage: Stage) {
    let (src, bindings) = compose_gsplat(stage).expect("composition succeeds");
    assert!(!bindings.is_empty(), "shader must declare engine bindings");
    let module = naga::front::wgsl::parse_str(&src)
        .unwrap_or_else(|e| panic!("parse failed:\n{}", e.emit_to_string(&src)));
    naga::valid::Validator::new(Default::default(), Default::default())
        .validate(&module)
        .unwrap_or_else(|e| panic!("validation failed: {e:?}"));
}

#[test]
fn gsplat_vertex_validates_with_naga() {
    validate(Stage::Vertex);
}

#[test]
fn gsplat_fragment_validates_with_naga() {
    validate(Stage::Fragment);
}

#[test]
fn pipeline_stages_share_one_binding_table() {
    let (vs, fs, unified) = countersplat_shaders::pipeline::compose_pipeline().expect("compose");
    for src in [&vs, &fs] {
        let module = naga::front::wgsl::parse_str(src).expect("parse");
        naga::valid::Validator::new(Default::default(), Default::default())
            .validate(&module)
            .expect("validate");
    }
    // Same name, same slot in both stages: alphaClipForward appears once in the union.
    let alpha: Vec<_> = unified
        .iter()
        .filter(|b| b.name == "alphaClipForward")
        .collect();
    assert_eq!(alpha.len(), 1);
    // No two resources share a (group, binding).
    let mut seen = std::collections::HashSet::new();
    for b in &unified {
        assert!(
            seen.insert((b.group, b.binding)),
            "duplicate slot for {}",
            b.name
        );
    }
}

#[test]
fn shared_centres_fit_portable_webgpu_storage_limits() {
    use countersplat_shaders::binding::BindingKind;
    let (_, _, bindings) = countersplat_shaders::pipeline::compose_pipeline().unwrap();
    assert!(
        bindings
            .iter()
            .filter(|b| matches!(b.kind, BindingKind::Storage { .. }))
            .count()
            <= 8
    );
    assert!(bindings.iter().any(|b| b.name == "splatCenters"));
    assert!(
        !bindings.iter().any(|b| matches!(
            b.name.as_str(),
            "means_l" | "means_u" | "means_mins" | "means_maxs"
        )),
        "rendering and sorting must share the same positions"
    );
    for stage in [Stage::Vertex, Stage::Fragment] {
        let (_, stage_bindings) = compose_gsplat(stage).unwrap();
        assert!(
            stage_bindings
                .iter()
                .filter(|b| matches!(b.kind, BindingKind::Uniform { .. }))
                .count()
                <= 12
        );
    }
}
