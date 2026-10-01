use crate::Result;
use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub(crate) struct FeedbackEntry {
    pub seq: i64,
    pub atom: String,
    pub action: String,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FeedbackResolution {
    pub history: Vec<FeedbackEntry>,
    pub latest_confirmation: Option<FeedbackEntry>,
    pub recall_suppressed: bool,
    pub memory_excluded: bool,
}

impl FeedbackResolution {
    pub fn corrections_json(&self) -> Vec<Value> {
        self.history
            .iter()
            .map(|entry| json!({"action":entry.action,"text":entry.text}))
            .collect()
    }

    pub fn has_action(&self, action: &str) -> bool {
        self.history.iter().any(|entry| entry.action == action)
    }
}

/// Resolve retained feedback history by its durable write sequence.
///
/// The two exclusion flags intentionally represent different policies:
/// recall omits `do_not_use` and `wrong_topic`, while research grouping and
/// prominence also omit `not_about_me` and `temporary_research`.
pub(crate) fn resolve_rows(
    entries: impl IntoIterator<Item = FeedbackEntry>,
) -> HashMap<String, FeedbackResolution> {
    let mut resolved = HashMap::<String, FeedbackResolution>::new();
    for entry in entries {
        let state = resolved.entry(entry.atom.clone()).or_default();
        state.recall_suppressed |= matches!(entry.action.as_str(), "do_not_use" | "wrong_topic");
        state.memory_excluded |= matches!(
            entry.action.as_str(),
            "do_not_use" | "wrong_topic" | "not_about_me" | "temporary_research"
        );
        if entry.action == "confirm_constraint"
            && state
                .latest_confirmation
                .as_ref()
                .is_none_or(|current| entry.seq > current.seq)
        {
            state.latest_confirmation = Some(entry.clone());
        }
        state.history.push(entry);
    }
    for state in resolved.values_mut() {
        state.history.sort_by_key(|entry| entry.seq);
    }
    resolved
}

/// Load histories only for a bounded set of known atom IDs. Chunking keeps the
/// query below SQLite's conservative bind-parameter limits while the
/// `(atom, action, seq)` index serves the lookup.
pub(crate) fn load_for_atoms(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, FeedbackResolution>> {
    const IDS_PER_QUERY: usize = 800;
    let mut entries = Vec::new();
    for chunk in ids.chunks(IDS_PER_QUERY) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT seq,atom,action,text FROM feedback \
             WHERE atom IN ({placeholders}) ORDER BY atom,seq"
        );
        let params = chunk.iter().map(String::as_str);
        for row in conn
            .prepare(&sql)?
            .query_map(params_from_iter(params), |row| {
                Ok(FeedbackEntry {
                    seq: row.get(0)?,
                    atom: row.get(1)?,
                    action: row.get(2)?,
                    text: row.get(3)?,
                })
            })?
        {
            entries.push(row?);
        }
    }
    Ok(resolve_rows(entries))
}
