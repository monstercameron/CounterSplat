//! The depth-prepass variant composes and validates, and binds `alphaClip` instead of `alphaClipForward`.
use countersplat_shaders::pipeline::compose_pipeline_with;

#[test]
fn prepass_variant_validates() {
    for defines in [
        &["PREPASS_PASS", "SH_BANDS=0"][..],
        &["PREPASS_PASS", "SH_BANDS=0", "GSPLAT_AA=1"][..],
    ] {
        let (vs, fs, bindings) = compose_pipeline_with(defines).expect("compose prepass");
        for (name, src) in [("vertex", &vs), ("fragment", &fs)] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
            naga::valid::Validator::new(Default::default(), naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
        let names: Vec<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"alphaClip"), "{names:?}");
        assert!(!names.contains(&"alphaClipForward"), "{names:?}");
    }
}
