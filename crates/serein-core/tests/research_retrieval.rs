//! Native regressions from the public-metadata research handoff.
//! The records are synthetic; no browser profile or user vault is accessed.
use rusqlite::params;
use serde_json::Value;
use serein_core::{id, now, storage::Vault, Policy, Recall};
use std::{ffi::OsString, path::Path, sync::Mutex};

static DATA_DIR_LOCK: Mutex<()> = Mutex::new(());
struct DataDirGuard(Option<OsString>);
impl DataDirGuard {
    fn set(path: &Path) -> Self {
        let prior = std::env::var_os("SEREIN_DATA_DIR");
        std::env::set_var("SEREIN_DATA_DIR", path);
        Self(prior)
    }
}
impl Drop for DataDirGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("SEREIN_DATA_DIR", value),
            None => std::env::remove_var("SEREIN_DATA_DIR"),
        }
    }
}
const SOURCE: &str = "11111111-1111-4111-8111-111111111111";
fn put(vault: &Vault, title: &str, query: Option<&str>, site: &str) -> String {
    let atom = id();
    vault.conn.execute(
        "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
         VALUES(?1,?2,?3,?4,?5,'visit',?6,?6,20,?1)",
        params![atom,SOURCE,site,title,query,now()],
    ).unwrap();
    atom
}
fn recall(vault: &mut Vault, question: &str, model: &Path) -> Value {
    vault
        .recall(
            &Recall {
                protocol: 1,
                request_id: id(),
                client: "research-regression".into(),
                vault: "default".into(),
                query: question.into(),
                facets: vec![],
                scope: vec!["research".into()],
                max_bytes: 4096,
                budget_ms: 2000,
            },
            SOURCE,
            model,
        )
        .unwrap()
}
fn ids(packet: &Value) -> Vec<&str> {
    packet["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect()
}
fn with_vault(test: impl FnOnce(&mut Vault, &Path)) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::tempdir().unwrap();
    let _guard = DataDirGuard::set(dir.path());
    let mut vault = Vault::open(&dir.path().join("vault")).unwrap();
    vault
        .set_policy(
            SOURCE,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    test(&mut vault, &dir.path().join("missing-model"));
}
#[test]
fn latin_substrings_do_not_manufacture_lexical_support() {
    with_vault(|v, m| {
        put(v, "Trust cart installation guide", None, "docs.example");
        let p = recall(v, "rust art installation", m);
        assert!(ids(&p).is_empty(), "{p}");
    });
}
#[test]
fn fts_diacritic_and_normalization_matches_keep_their_lexical_support() {
    with_vault(|v, m| {
        let atom = put(v, "Café preparation", None, "coffee.example");
        for question in ["cafe", "café", "cafe\u{301}"] {
            let p = recall(v, question, m);
            assert!(ids(&p).contains(&atom.as_str()), "question={question}: {p}");
        }
    });
}
#[test]
fn short_entities_remain_searchable_without_short_stopword_noise() {
    with_vault(|v, m| {
        let uv = put(v, "uv", None, "docs.example");
        let m3 = put(v, "M3 processor specifications", None, "hardware.example");
        for (question, atom) in [("uv", uv), ("M3", m3)] {
            let p = recall(v, question, m);
            assert!(ids(&p).contains(&atom.as_str()), "question={question}: {p}");
        }
        for question in ["is it", "to be", "and the"] {
            let p = recall(v, question, m);
            assert!(ids(&p).is_empty(), "question={question}: {p}");
        }
    });
}
#[test]
fn comparison_local_memory_sizes_do_not_leak_between_arms() {
    with_vault(|v, m| {
        let first = put(v, "X1 16GB laptop specification", None, "first.example");
        let second = put(v, "X2 32GB laptop specification", None, "second.example");
        for question in [
            "Compare X1 16GB and X2 32GB laptops",
            "Compare X2 32GB and X1 16GB laptops",
        ] {
            let p = recall(v, question, m);
            let got = ids(&p);
            assert!(
                got.contains(&first.as_str()) && got.contains(&second.as_str()),
                "{p}"
            );
        }
    });
}
#[test]
fn bare_short_model_comparison_preserves_both_arms() {
    with_vault(|v, m| {
        let first = put(v, "X1 laptop specification", None, "first.example");
        let second = put(v, "X2 laptop specification", None, "second.example");
        let p = recall(v, "Compare X1 and X2", m);
        let got = ids(&p);
        assert!(
            got.contains(&first.as_str()) && got.contains(&second.as_str()),
            "{p}"
        );
    });
}
#[test]
fn cjk_character_phrases_include_two_character_queries() {
    with_vault(|v, m| {
        let bonsai = put(v, "盆栽の育て方", None, "garden.example");
        let balcony = put(v, "ベランダ園芸の水やりガイド", None, "balcony.example");
        for (question, atom) in [("盆栽", bonsai), ("ベランダ園芸", balcony)] {
            let p = recall(v, question, m);
            assert!(ids(&p).contains(&atom.as_str()), "question={question}: {p}");
        }
        assert!(ids(&recall(v, "盆地", m)).is_empty());
    });
}

#[test]
fn emitted_search_evidence_includes_the_title_used_to_support_an_exact_value() {
    with_vault(|v, m| {
        put(
            v,
            "Laptop 16GB specifications",
            Some("laptop specifications"),
            "hardware.example",
        );
        let p = recall(v, "laptop 16GB specifications", m);
        let records = p["context"].as_array().unwrap();
        assert!(!records.is_empty(), "{p}");
        assert!(records[0]["text"].as_str().unwrap().contains("16GB"), "{p}");
    });
}

#[test]
fn bare_model_punctuation_aliases_are_discovered_in_both_directions() {
    with_vault(|v, m| {
        let camera = put(v, "RX-100", None, "camera.example");
        let second = put(v, "ZV1", None, "camera.example");
        for (question, atom) in [("RX100", camera), ("ZV-1", second)] {
            let p = recall(v, question, m);
            assert!(ids(&p).contains(&atom.as_str()), "question={question}: {p}");
        }
    });
}

#[test]
fn amount_codes_and_symbols_share_only_unambiguous_currency_identity() {
    with_vault(|v, m| {
        let euro = put(v, "Chair €500 offer", None, "furniture.example");
        for question in ["chair 500 EUR", "chair EUR 500"] {
            let p = recall(v, question, m);
            assert!(ids(&p).contains(&euro.as_str()), "question={question}: {p}");
        }
        assert!(ids(&recall(v, "chair 500 USD", m)).is_empty());
    });
}

#[test]
fn unsupported_numeric_semantics_are_visible_without_returning_broader_evidence() {
    with_vault(|v, m| {
        put(v, "Camera $500 lens", None, "camera.example");
        for question in ["camera under $500", "camera $1,200.00.99"] {
            let p = recall(v, question, m);
            assert!(ids(&p).is_empty(), "{p}");
            assert_eq!(p["status"], "partial", "{p}");
            assert!(
                p["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|w| w.as_str().unwrap().contains("cannot interpret")),
                "{p}"
            );
        }
    });
}

#[test]
fn an_infinitive_preamble_preserves_a_single_direction() {
    with_vault(|v, m| {
        let forward = put(
            v,
            "migration from Linux to Windows",
            None,
            "forward.example",
        );
        let reverse = put(
            v,
            "migration from Windows to Linux",
            None,
            "reverse.example",
        );
        for (question, atom) in [
            ("How to migrate from Linux to Windows", forward),
            ("How to migrate from Windows to Linux", reverse),
        ] {
            let p = recall(v, question, m);
            assert_eq!(
                ids(&p).first().copied(),
                Some(atom.as_str()),
                "question={question}: {p}"
            );
        }
    });
}
