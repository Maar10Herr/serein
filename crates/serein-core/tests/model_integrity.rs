use serde_json::Value;
use serein_core::{hash, id, model::Encoder, now, storage::Vault, Event, Policy, Recall};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

fn copy_pack(destination: &Path) -> PathBuf {
    fs::create_dir_all(destination).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    for name in [
        "manifest.json",
        "weights.i8",
        "scales.f32",
        "tokenizer.json",
    ] {
        fs::copy(source.join(name), destination.join(name)).unwrap();
    }
    destination.to_path_buf()
}

fn flip_first_byte(path: &Path) -> u8 {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut original = [0];
    file.read_exact(&mut original).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[original[0] ^ 0xff]).unwrap();
    original[0]
}

fn restore_first_byte(path: &Path, original: u8) {
    let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[original]).unwrap();
}

#[test]
fn model_payload_digests_reject_corruption_and_recall_falls_back_to_lexical() {
    let temp = tempfile::tempdir().unwrap();
    let pack = copy_pack(&temp.path().join("model"));
    let manifest: Value =
        serde_json::from_slice(&fs::read(pack.join("manifest.json")).unwrap()).unwrap();

    for (file, digest) in [
        ("weights.i8", "weights_sha256"),
        ("scales.f32", "scales_sha256"),
        ("tokenizer.json", "tokenizer_sha256"),
    ] {
        let bytes = fs::read(pack.join(file)).unwrap();
        assert_eq!(hash(&bytes), manifest[digest].as_str().unwrap(), "{file}");
    }
    assert!(
        Encoder::open(&pack).is_ok(),
        "unchanged verified pack opens"
    );

    for file in ["weights.i8", "scales.f32", "tokenizer.json"] {
        let path = pack.join(file);
        let original = flip_first_byte(&path);
        let error = Encoder::open(&pack)
            .err()
            .unwrap_or_else(|| panic!("corrupt {file} was accepted"));
        assert_eq!(error.0, "MODEL_INVALID", "{file}");
        assert!(error.1.contains("checksum mismatch"), "{file}: {}", error.1);
        restore_first_byte(&path, original);
    }
    assert!(
        Encoder::open(&pack).is_ok(),
        "restoring payload bytes restores validity"
    );

    let weights = pack.join("weights.i8");
    let original = flip_first_byte(&weights);
    let source = id();
    let mut vault = Vault::open(&temp.path().join("context.sqlite")).unwrap();
    vault
        .set_policy(
            &source,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    vault
        .ingest(
            &source,
            1,
            vec![Event {
                event_id: id(),
                visit_id: id(),
                site_key: "example.com".into(),
                site_epoch: 0,
                observed_at: now(),
                kind: "search".into(),
                title: "Desk lamp research".into(),
                search_query: Some("desk lamp research".into()),
                foreground_seconds: 10,
            }],
        )
        .unwrap();
    let response = vault
        .recall(
            &Recall {
                protocol: 1,
                request_id: id(),
                client: "model-integrity-test".into(),
                vault: "default".into(),
                query: "desk lamp".into(),
                facets: vec![],
                scope: vec!["research".into()],
                max_bytes: 4096,
                budget_ms: 2000,
            },
            &source,
            &pack,
        )
        .unwrap();
    assert_eq!(response["index"]["mode"], "lexical");
    assert_eq!(response["status"], "partial");
    assert_eq!(response["context"].as_array().unwrap().len(), 1);
    restore_first_byte(&weights, original);
}
