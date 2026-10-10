//! Native recall regressions for the narrow query and exact-value audit cases.
//! Each fixture uses a disposable Vault and the public ingest/recall APIs.

use serde_json::Value;
use serein_core::{id, now, storage::Vault, Event, Policy, Recall};
use std::path::Path;
use tempfile::TempDir;

const MODEL_PATH: &str = "/no/model/for/research-query-contract-tests";

struct Fixture {
    vault: Vault,
    _directory: TempDir,
    source: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create disposable vault directory");
        let source = id();
        let mut vault = Vault::open(&directory.path().join("research-query.sqlite"))
            .expect("open disposable vault");
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
            .expect("enable isolated test source");
        Self {
            vault,
            _directory: directory,
            source,
        }
    }

    fn observe(&mut self, title: &str, site: &str) {
        let event = Event {
            event_id: id(),
            visit_id: id(),
            site_key: site.to_owned(),
            site_epoch: 0,
            observed_at: now(),
            kind: "visit".to_owned(),
            title: title.to_owned(),
            search_query: None,
            foreground_seconds: 30,
        };
        let result = self
            .vault
            .ingest(&self.source, 1, vec![event])
            .expect("ingest public metadata into disposable vault");
        assert_eq!(result["acknowledged_ids"].as_array().map(Vec::len), Some(1));
    }

    fn observe_confirmed(&mut self, title: &str, site: &str, confirmation: &str) {
        self.observe(title, site);
        let dashboard = self
            .vault
            .dashboard(&self.source)
            .expect("read isolated dashboard");
        let atom = dashboard["cards"]
            .as_array()
            .expect("dashboard cards")
            .iter()
            .find(|card| card["text"] == title && card["site"] == site)
            .and_then(|card| card["id"].as_str())
            .expect("find atom created by the observation")
            .to_owned();
        self.vault
            .feedback(
                &self.source,
                &atom,
                "confirm_constraint",
                Some(confirmation),
            )
            .expect("confirm the fixture decision through the public API");
    }

    fn recall(&mut self, query: &str, scope: &[&str]) -> Value {
        let request = Recall {
            protocol: 1,
            request_id: id(),
            client: "research-query-contract-test".to_owned(),
            vault: "default".to_owned(),
            query: query.to_owned(),
            facets: Vec::new(),
            scope: scope.iter().map(|scope| (*scope).to_owned()).collect(),
            max_bytes: 4096,
            budget_ms: 1500,
        };
        self.vault
            .recall(&request, &self.source, Path::new(MODEL_PATH))
            .expect("recall from disposable vault")
    }
}

fn context_texts(packet: &Value) -> Vec<String> {
    packet["context"]
        .as_array()
        .expect("recall packet has a context array")
        .iter()
        .filter_map(|record| record["text"].as_str().map(str::to_owned))
        .collect()
}

fn contains_text(packet: &Value, needle: &str) -> bool {
    context_texts(packet)
        .iter()
        .any(|text| text.to_lowercase().contains(&needle.to_lowercase()))
}

#[test]
fn how_much_ram_is_a_quantity_not_a_monetary_amount() {
    let mut fixture = Fixture::new();
    fixture.observe("Laptop with 16GB RAM specifications", "memory.example.com");

    let ram = fixture.recall("How much RAM does the laptop have?", &["research"]);
    assert!(
        contains_text(&ram, "16GB RAM"),
        "a storage quantity should answer a RAM question: {ram}"
    );
}

#[test]
fn weight_query_does_not_accept_a_data_quantity() {
    let mut fixture = Fixture::new();
    fixture.observe(
        "Laptop weight specification also lists 16GB RAM",
        "memory.example.com",
    );
    fixture.observe("Laptop weight 2kg", "weight.example.com");

    let packet = fixture.recall("How much weight is the laptop?", &["research"]);
    let texts = context_texts(&packet);
    assert!(
        texts.iter().any(|text| text.contains("2kg")),
        "a mass value should answer a weight question: {packet}"
    );
    assert!(
        texts.iter().all(|text| !text.contains("16GB")),
        "a data capacity cannot answer a mass question: {packet}"
    );
}

#[test]
fn pound_price_answers_a_monetary_question() {
    let mut fixture = Fixture::new();
    fixture.observe("Office chair cost £499", "chair.example.com");
    let price = fixture.recall("How much does the office chair cost?", &["research"]);
    assert!(
        contains_text(&price, "£499"),
        "a pound-denominated price should answer a cost question: {price}"
    );
}

#[test]
fn exact_money_keeps_the_complete_fractional_value() {
    let mut fixture = Fixture::new();
    fixture.observe("Laptop price $1,200.99", "wrong-price.example.com");

    let packet = fixture.recall("Laptop price $1,200.00", &["research"]);
    assert!(
        !contains_text(&packet, "$1,200.99"),
        "different cents must not satisfy an exact price requirement: {packet}"
    );
}

#[test]
fn currency_only_qualifiers_do_not_accept_an_unqualified_usd_price() {
    let mut fixture = Fixture::new();
    for code in ["CHF", "EUR"] {
        fixture.observe(
            &format!("Laptop cost comparison in {code} with listed price USD 500"),
            &format!("{}.example.com", code.to_lowercase()),
        );
        let packet = fixture.recall(
            &format!("What does the laptop cost in {code}?"),
            &["research"],
        );
        assert!(
            !contains_text(&packet, "USD 500"),
            "a currency-only qualifier must not accept USD evidence: {code}: {packet}"
        );
    }
}

#[test]
fn exact_eur_amount_remains_comparable_after_qualifier_guard() {
    let mut fixture = Fixture::new();
    fixture.observe("Laptop price EUR 500", "eur.example.com");
    fixture.observe("Laptop price USD 500", "usd.example.com");

    let packet = fixture.recall("What does the laptop cost? EUR 500", &["research"]);
    let texts = context_texts(&packet);
    assert!(
        texts.iter().any(|text| text.contains("EUR 500")),
        "an exact EUR amount remains supported: {packet}"
    );
    assert!(
        texts.iter().all(|text| !text.contains("USD 500")),
        "an exact EUR amount must reject a USD amount: {packet}"
    );
}

#[test]
fn stored_from_to_range_is_not_answered_as_two_exact_equalities() {
    let mut fixture = Fixture::new();
    fixture.observe("Laptop price from €500 to €1000", "laptop.example.com");

    let packet = fixture.recall(
        "What does the laptop cost from €500 to €1000?",
        &["research"],
    );
    assert!(
        !contains_text(&packet, "from €500 to €1000"),
        "unsupported ranges must not be answered as a pair of equalities: {packet}"
    );
}

#[test]
fn data_exact_values_distinguish_bytes_from_bits() {
    let mut fixture = Fixture::new();
    fixture.observe("Portable drive 16Gb capacity", "bits.example.com");
    fixture.observe("Portable drive 16GB capacity", "bytes.example.com");

    let packet = fixture.recall("Portable drive 16GB capacity", &["research"]);
    let texts = context_texts(&packet);
    assert!(
        texts.iter().any(|text| text.contains("16GB")),
        "the byte value should satisfy the exact query: {packet}"
    );
    assert!(
        texts.iter().all(|text| !text.contains("16Gb")),
        "bits and bytes are different exact values: {packet}"
    );
}

#[test]
fn connector_revision_is_exact() {
    let mut fixture = Fixture::new();
    fixture.observe("USB-A docking station", "usb-a.example.com");
    fixture.observe("USB-C docking station", "usb-c.example.com");

    let connector = fixture.recall("USB-C docking station", &["research"]);
    assert!(
        contains_text(&connector, "USB-C"),
        "exact connector should match: {connector}"
    );
    assert!(
        !contains_text(&connector, "USB-A"),
        "a different connector revision must not match: {connector}"
    );
}

#[test]
fn hyphenated_model_spelling_matches_the_same_model_identifier() {
    let mut fixture = Fixture::new();
    fixture.observe("RX-100 camera specifications", "camera.example.com");
    let model = fixture.recall("RX100 camera", &["research"]);
    assert!(
        contains_text(&model, "RX-100"),
        "a punctuation-only model alias should match: {model}"
    );
}

#[test]
fn select_paraphrase_requires_confirmation() {
    let mut fixture = Fixture::new();
    fixture.observe(
        "Ergonomic office chair research",
        "chair-research.example.com",
    );

    let packet = fixture.recall("Which ergonomic office chair did I select?", &["research"]);
    assert!(
        !contains_text(&packet, "Ergonomic office chair research"),
        "observed research cannot certify a personal selection: {packet}"
    );
}

#[test]
fn order_paraphrase_requires_confirmation() {
    let mut fixture = Fixture::new();
    fixture.observe(
        "Ergonomic office chair research",
        "chair-research.example.com",
    );
    let packet = fixture.recall("Did I order the ergonomic office chair?", &["research"]);
    assert!(
        !contains_text(&packet, "Ergonomic office chair research"),
        "observed research cannot certify a personal order: {packet}"
    );
}

#[test]
fn choose_remains_a_confirmation_required_control() {
    let mut fixture = Fixture::new();
    fixture.observe(
        "Ergonomic office chair research",
        "chair-research.example.com",
    );
    let packet = fixture.recall("Which ergonomic office chair did I choose?", &["research"]);
    assert!(
        !contains_text(&packet, "Ergonomic office chair research"),
        "the existing choose gate must remain: {packet}"
    );
}

#[test]
fn observed_public_price_cannot_answer_how_much_did_i_spend() {
    let mut fixture = Fixture::new();
    fixture.observe("Office chair price £499", "chair.example.com");

    let packet = fixture.recall(
        "How much did I spend on the office chair?",
        &["research", "confirmed_preferences"],
    );
    assert!(
        !contains_text(&packet, "£499"),
        "an observed public price cannot establish personal expenditure: {packet}"
    );
}

#[test]
fn confirmed_spend_can_answer_the_matching_personal_spending_question() {
    let mut fixture = Fixture::new();
    fixture.observe_confirmed(
        "Office chair purchase record £499",
        "chair.example.com",
        "I spent £499 on the office chair",
    );

    let packet = fixture.recall(
        "How much did I spend on the office chair?",
        &["confirmed_preferences"],
    );
    assert!(
        contains_text(&packet, "I spent £499"),
        "a current matching confirmation can support a personal spending answer: {packet}"
    );
}

#[test]
fn confirmed_research_duration_can_answer_how_much_time_i_spent() {
    let mut fixture = Fixture::new();
    fixture.observe_confirmed(
        "Heat pump water heater research notes 12 minutes",
        "heat-pump.example.com",
        "I spent 12 minutes researching the heat pump water heater",
    );

    let packet = fixture.recall(
        "How much time did I spend researching the heat pump water heater?",
        &["confirmed_preferences"],
    );
    assert!(
        contains_text(&packet, "12 minutes"),
        "a matching current duration confirmation should answer a time question: {packet}"
    );
}

#[test]
fn current_confirmed_selection_and_order_can_answer_their_own_queries() {
    let mut fixture = Fixture::new();
    fixture.observe_confirmed(
        "Ergonomic office chair research",
        "chair-research.example.com",
        "I selected the ergonomic office chair",
    );
    fixture.observe_confirmed(
        "USB-C dock research",
        "dock-research.example.com",
        "I ordered the USB-C dock",
    );

    let selection = fixture.recall(
        "Which ergonomic office chair did I select?",
        &["confirmed_preferences"],
    );
    assert!(
        contains_text(&selection, "I selected the ergonomic office chair"),
        "the current matching confirmation should answer a selection question: {selection}"
    );

    let order = fixture.recall("Did I order the USB-C dock?", &["confirmed_preferences"]);
    assert!(
        contains_text(&order, "I ordered the USB-C dock"),
        "the current matching confirmation should answer an order question: {order}"
    );
}

#[test]
fn ordinary_research_question_can_return_observed_evidence() {
    let mut fixture = Fixture::new();
    fixture.observe(
        "Ergonomic office chair research",
        "chair-research.example.com",
    );
    let research = fixture.recall("What ergonomic office chair did I research?", &["research"]);
    assert!(
        contains_text(&research, "Ergonomic office chair research"),
        "an ordinary research question may return observed evidence: {research}"
    );
}
