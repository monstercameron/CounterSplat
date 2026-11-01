//! Validate the zero-centroid shortcut and the all-SH comparison shader.
use countersplat_shaders::pipeline::compose_pipeline_with;
use naga::{AddressSpace, Expression, ShaderStage};

fn validate(src: &str) -> naga::Module {
    let module =
        naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
    naga::valid::Validator::new(Default::default(), Default::default())
        .validate(&module)
        .expect("shader validation");
    module
}

#[test]
fn candidate_and_reference_have_the_same_bindings_and_valid_sh_variants() {
    for extra in [
        &[][..],
        &["GSPLAT_AA=1"][..],
        &["SH_BANDS=1"][..],
        &["SH_BANDS=2"][..],
    ] {
        let mut reference_extra = extra.to_vec();
        reference_extra.push("SPLAT_FULL_SH_REFERENCE");
        let (vs, fs, bindings) = compose_pipeline_with(extra).expect("candidate composition");
        let (ref_vs, ref_fs, ref_bindings) =
            compose_pipeline_with(&reference_extra).expect("reference composition");
        assert_eq!(
            bindings, ref_bindings,
            "reference must use the same resource layout"
        );
        assert_eq!(
            bindings
                .iter()
                .filter(|b| b.name == "sh_zero_reserved")
                .count(),
            1
        );
        let candidate = validate(&vs);
        let reference = validate(&ref_vs);
        validate(&fs);
        validate(&ref_fs);

        // The reference declares the same capability uniform but does not read it.
        let uses_capability = |module: &naga::Module| {
            let (handle, var) = module
                .global_variables
                .iter()
                .find(|(_, var)| var.name.as_deref() == Some("sh_zero_reserved"))
                .expect("capability uniform");
            assert_eq!(var.space, AddressSpace::Uniform);
            let vertex = module
                .entry_points
                .iter()
                .find(|entry| entry.stage == ShaderStage::Vertex)
                .unwrap();
            vertex
                .function
                .expressions
                .iter()
                .any(|(_, expr)| matches!(expr, Expression::GlobalVariable(h) if *h == handle))
        };
        assert!(uses_capability(&candidate));
        assert!(!uses_capability(&reference));
    }
}

#[test]
fn depth_prepass_does_not_need_the_sh_capability() {
    for extra in [
        &["PREPASS_PASS", "SH_BANDS=0"][..],
        &["PREPASS_PASS", "SH_BANDS=0", "GSPLAT_AA=1"][..],
    ] {
        let (vs, fs, bindings) = compose_pipeline_with(extra).expect("prepass composition");
        validate(&vs);
        validate(&fs);
        assert!(!bindings
            .iter()
            .any(|binding| binding.name == "sh_zero_reserved"));
    }
}
