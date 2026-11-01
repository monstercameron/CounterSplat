//! Validate local GPU kernels even when a graphics adapter is unavailable.

#[test]
fn local_renderer_shaders_parse_and_validate() {
    for (name, source) in [
        ("depth keys", include_str!("../src/shaders/depth_keys.wgsl")),
        (
            "draw arguments",
            include_str!("../src/shaders/draw_args.wgsl"),
        ),
        ("sky", include_str!("../src/shaders/sky.wgsl")),
    ] {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
        naga::valid::Validator::new(Default::default(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    }
}
