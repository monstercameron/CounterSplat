use countersplat_scene::CameraLens;
use serde::Deserialize;
use serde_json::json;

#[test]
fn borrowed_lens_decoding_matches_owned_decoding() {
    let mut full = serde_json::to_value(CameraLens::default()).unwrap();
    for value in [
        full.clone(),
        json!({}),
        json!({"aperture_f_number":4.0,"depth_of_field":true}),
        json!({"aperture_f_number":"invalid"}),
        json!({"shutter_fps":null}),
        json!({"unknown_field":1}),
        json!(null),
        json!([]),
    ] {
        let owned = serde_json::from_value::<CameraLens>(value.clone());
        let borrowed = CameraLens::deserialize(&value);
        match (owned, borrowed) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(_), Err(_)) => {}
            (a, b) => panic!("decoders disagree: {a:?} vs {b:?}"),
        }
    }
    full["iso"] = json!(1600.0);
    assert_eq!(
        serde_json::from_value::<CameraLens>(full.clone()).unwrap(),
        CameraLens::deserialize(&full).unwrap()
    );
}
