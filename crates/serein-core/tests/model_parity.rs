use serde_json::Value;
use serein_core::model::{cosine, Encoder};
#[test]
fn rust_encoder_matches_release_converter() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    let enc = Encoder::open(&path).expect("real prepared pack must be present for acceptance");
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(path.join("parity.json")).unwrap()).unwrap();
    for example in fixture["examples"].as_array().unwrap() {
        let text = example["text"].as_str().unwrap();
        let ids: Vec<u32> = serde_json::from_value(example["token_ids"].clone()).unwrap();
        assert_eq!(enc.token_ids(text).unwrap(), ids);
        let expected: Vec<f32> =
            serde_json::from_value(example["expected_vector_f32"].clone()).unwrap();
        let actual = enc.encode(text).unwrap();
        assert!(cosine(&actual, &expected) > 0.999999);
        assert!(actual
            .iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() < 1e-6));
    }
    assert!(enc.encode("").is_none());
}
#[test]
fn corrupt_pack_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("manifest.json"), b"{}").unwrap();
    assert!(Encoder::open(d.path()).is_err())
}
