use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub protocol: u32,
    pub request_id: String,
    pub source_id: String,
    pub op: String,
    pub capture_epoch: u64,
    pub payload: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event_id: String,
    pub visit_id: String,
    pub site_key: String,
    pub site_epoch: u64,
    pub observed_at: String,
    pub kind: String,
    pub title: String,
    pub search_query: Option<String>,
    pub foreground_seconds: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recall {
    pub protocol: u32,
    pub request_id: String,
    pub client: String,
    pub vault: String,
    pub query: String,
    #[serde(default)]
    pub facets: Vec<String>,
    pub scope: Vec<String>,
    pub max_bytes: usize,
    pub budget_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub consent: bool,
    pub paused: bool,
    pub recall_enabled: bool,
    pub selected_only: bool,
    pub selected_sites: Vec<String>,
    pub excluded_sites: Vec<String>,
    pub capture_epoch: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            consent: false,
            paused: false,
            recall_enabled: false,
            selected_only: false,
            selected_sites: vec![],
            excluded_sites: vec![],
            capture_epoch: 0,
        }
    }
}
impl Envelope {
    pub fn validate(&self) -> Result<()> {
        if self.protocol != 1
            || ![
                "hello",
                "ingest",
                "status",
                "dashboard",
                "policy.update",
                "forget",
                "feedback",
                "receipts",
            ]
            .contains(&self.op.as_str())
        {
            return Err(invalid("Unsupported protocol or operation."));
        }
        if !self.payload.is_object() {
            return Err(invalid("Operation payload must be an object."));
        }
        if ["status", "dashboard", "receipts"].contains(&self.op.as_str())
            && !self.payload.as_object().unwrap().is_empty()
        {
            return Err(invalid("This operation accepts no payload fields."));
        }
        if self.op == "hello"
            && (self.payload.as_object().unwrap().len() != 1 || !self.payload["nonce"].is_string())
        {
            return Err(invalid("Hello requires only a pairing nonce."));
        }
        check_id(&self.request_id)?;
        check_id(&self.source_id)
    }
}
impl Event {
    pub fn validate(&self) -> Result<()> {
        check_id(&self.event_id)?;
        check_id(&self.visit_id)?;
        let time = chrono::DateTime::parse_from_rfc3339(&self.observed_at)
            .map_err(|_| invalid("Invalid timestamp."))?;
        let age = chrono::Utc::now().signed_duration_since(time).num_seconds();
        if !(-300..=90 * 86400).contains(&age)
            || self.foreground_seconds > 3600
            || !["visit", "search"].contains(&self.kind.as_str())
            || self.title.chars().count() > 256
            || self
                .search_query
                .as_ref()
                .is_some_and(|x| x.chars().count() > 512)
            || self.title.len() + self.search_query.as_ref().map_or(0, |x| x.len()) > 2048
            || !policy::valid_site(&self.site_key)
        {
            return Err(invalid("Invalid observation fields."));
        }
        Ok(())
    }
}
impl Recall {
    pub fn validate(&self) -> Result<()> {
        check_id(&self.request_id)?;
        if self.protocol != 1
            || self.query.chars().count() > 512
            || self.facets.len() > 3
            || self.facets.iter().any(|f| f.chars().count() > 128)
            || !(512..=16384).contains(&self.max_bytes)
            || self.client.len() > 64
            || self
                .scope
                .iter()
                .any(|s| !["projects", "research", "confirmed_preferences"].contains(&s.as_str()))
        {
            return Err(invalid("Invalid recall limits or scope."));
        }
        if self.vault != "default" {
            check_id(&self.vault)?
        }
        Ok(())
    }
}
