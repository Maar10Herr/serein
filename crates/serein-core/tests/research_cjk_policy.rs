//! Policy-boundary regressions for supplemental CJK recall.
//!
//! These tests use only synthetic rows in a disposable native Vault. They exercise
//! recall and feedback through the public API; direct inserts make source and age
//! fixtures deterministic without touching a user's browser data.
use rusqlite::params;
use serde_json::Value;
use serein_core::{id, now, storage::Vault, Policy, Recall};
use std::{ffi::OsString, path::Path, sync::Mutex};

static DATA_DIR_LOCK: Mutex<()> = Mutex::new(());
const SOURCE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_SOURCE: &str = "22222222-2222-4222-8222-222222222222";

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

fn enabled_policy() -> Policy {
    Policy {
        consent: true,
        recall_enabled: true,
        capture_epoch: 1,
        ..Policy::default()
    }
}

fn with_vault(test: impl FnOnce(&mut Vault, &Path)) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::tempdir().unwrap();
    let _guard = DataDirGuard::set(dir.path());
    let mut vault = Vault::open(&dir.path().join("vault")).unwrap();
    vault.set_policy(SOURCE, &enabled_policy()).unwrap();
    test(&mut vault, &dir.path().join("missing-model"));
}

fn put(vault: &Vault, source: &str, site: &str, title: &str, last_seen: &str) -> String {
    let atom = id();
    vault
        .conn
        .execute(
            "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
             VALUES(?1,?2,?3,?4,NULL,'visit',?5,?6,20,?1)",
            params![atom, source, site, title, now(), last_seen],
        )
        .unwrap();
    atom
}

fn recall(vault: &mut Vault, question: &str, scope: &[&str], model: &Path) -> Value {
    vault
        .recall(
            &Recall {
                protocol: 1,
                request_id: id(),
                client: "research-cjk-policy-test".into(),
                vault: "default".into(),
                query: question.into(),
                facets: vec![],
                scope: scope.iter().map(|scope| (*scope).to_owned()).collect(),
                max_bytes: 4096,
                budget_ms: 2000,
            },
            SOURCE,
            model,
        )
        .unwrap()
}

fn packet_ids(packet: &Value) -> Vec<&str> {
    packet["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect()
}

#[test]
fn cjk_supplemental_recall_obeys_source_retention_and_suppression() {
    with_vault(|vault, model| {
        let retained = put(
            vault,
            SOURCE,
            "garden.example",
            "Research notes: 盆栽の育て方",
            &now(),
        );
        let other_source = put(
            vault,
            OTHER_SOURCE,
            "garden.example",
            "Research notes: 盆栽の剪定",
            &now(),
        );
        let stale = put(
            vault,
            SOURCE,
            "garden.example",
            "Research notes: 盆栽の植え替え",
            "2020-01-01T00:00:00Z",
        );
        let wrong_topic = put(
            vault,
            SOURCE,
            "garden.example",
            "Research notes: 盆栽の水やり",
            &now(),
        );
        let do_not_use = put(
            vault,
            SOURCE,
            "garden.example",
            "Research notes: 盆栽の鉢選び",
            &now(),
        );

        vault
            .feedback(SOURCE, &wrong_topic, "wrong_topic", None)
            .unwrap();
        vault
            .feedback(SOURCE, &do_not_use, "do_not_use", None)
            .unwrap();

        let packet = recall(vault, "盆栽", &["research"], model);
        let got = packet_ids(&packet);
        assert!(got.contains(&retained.as_str()), "{packet}");
        for excluded in [other_source, stale, wrong_topic, do_not_use] {
            assert!(
                !got.contains(&excluded.as_str()),
                "ineligible CJK evidence {excluded} leaked: {packet}"
            );
        }
    });
}

#[test]
fn cjk_supplemental_recall_obeys_selected_and_excluded_sites() {
    with_vault(|vault, model| {
        let allowed = put(
            vault,
            SOURCE,
            "allowed.example",
            "Research notes: 盆栽の育て方",
            &now(),
        );
        let unselected = put(
            vault,
            SOURCE,
            "unselected.example",
            "Research notes: 盆栽の剪定",
            &now(),
        );
        let explicitly_excluded = put(
            vault,
            SOURCE,
            "blocked.example",
            "Research notes: 盆栽の水やり",
            &now(),
        );

        vault
            .set_policy(
                SOURCE,
                &Policy {
                    consent: true,
                    paused: false,
                    recall_enabled: true,
                    selected_only: true,
                    selected_sites: vec!["allowed.example".into(), "blocked.example".into()],
                    excluded_sites: vec!["blocked.example".into()],
                    capture_epoch: 2,
                },
            )
            .unwrap();

        let packet = recall(vault, "盆栽", &["research"], model);
        let got = packet_ids(&packet);
        assert!(got.contains(&allowed.as_str()), "{packet}");
        for excluded in [unselected, explicitly_excluded] {
            assert!(
                !got.contains(&excluded.as_str()),
                "site-ineligible CJK evidence {excluded} leaked: {packet}"
            );
        }
    });
}

#[test]
fn cjk_supplemental_recall_uses_only_the_latest_confirmation() {
    with_vault(|vault, model| {
        let atom = put(
            vault,
            SOURCE,
            "garden.example",
            "Research notes: 盆栽の育て方",
            &now(),
        );
        vault
            .feedback(SOURCE, &atom, "confirm_constraint", Some("盆栽の育て方"))
            .unwrap();
        vault
            .feedback(
                SOURCE,
                &atom,
                "confirm_constraint",
                Some("Oak desk for office"),
            )
            .unwrap();

        let observed = recall(vault, "盆栽", &["research"], model);
        assert!(
            !packet_ids(&observed).contains(&atom.as_str()),
            "confirmed evidence remained in observed research scope: {observed}"
        );

        let replaced = recall(vault, "盆栽", &["confirmed_preferences"], model);
        assert!(
            !packet_ids(&replaced).contains(&atom.as_str()),
            "older confirmation or observed title survived replacement: {replaced}"
        );

        let latest = recall(vault, "oak desk", &["confirmed_preferences"], model);
        assert!(
            packet_ids(&latest).contains(&atom.as_str()),
            "latest confirmation was not searchable: {latest}"
        );
    });
}

#[test]
fn cjk_supplemental_query_uses_the_bounded_public_token_lane() {
    with_vault(|vault, model| {
        let phrases = [
            "甲乙", "丙丁", "戊己", "庚辛", "壬癸", "子丑", "寅卯", "辰巳", "午未", "申酉", "戌亥",
            "春夏", "秋冬", "东西", "南北", "左右", "上下", "前后", "早晚", "多少", "高低", "长短",
            "新旧", "大小", "真假", "黑白", "天地", "山川", "花草", "鸟兽", "鱼虫", "江河", "湖海",
            "风雨",
        ];
        let question = phrases.join(" ");
        assert!(question.chars().count() <= 512);

        // The public query lane keeps the first 32 unique terms. Sixteen match
        // only as literal substrings of longer CJK runs; that is exactly half
        // of the bounded 32-term query contract and would be below half of all 34.
        let title = format!(
            "Research notes: {}",
            phrases[..16]
                .iter()
                .map(|phrase| format!("{phrase}的"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let atom = put(vault, SOURCE, "research.example", &title, &now());
        let packet = recall(vault, &question, &["research"], model);
        assert!(
            packet_ids(&packet).contains(&atom.as_str()),
            "bounded public CJK query failed: {packet}"
        );
    });
}
