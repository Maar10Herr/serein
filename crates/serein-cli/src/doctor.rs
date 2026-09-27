use rusqlite::{Connection as SqliteConnection, OpenFlags};
use serde_json::{json, Value};
use serein_core::{hash, install, model, Policy, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn check(state: &str, detail: &str, remedy: &str) -> Value {
    json!({"state":state,"detail":detail,"remedy":remedy})
}

fn native_host(root: &Path, browser: &str, extension_id: &str) -> Value {
    let receipt_path = root.join("receipts").join(format!("native-{browser}.json"));
    let valid = (|| -> Option<bool> {
        let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).ok()?).ok()?;
        let path = PathBuf::from(receipt["path"].as_str()?);
        let bytes = fs::read(path).ok()?;
        if hash(&bytes) != receipt["sha256"].as_str()? || receipt["browser"] != browser {
            return Some(false);
        }
        let manifest: Value = serde_json::from_slice(&bytes).ok()?;
        let host = PathBuf::from(manifest["path"].as_str()?);
        let caller = if browser == "firefox" {
            manifest["allowed_extensions"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == extension_id))
        } else {
            let origin = format!("chrome-extension://{extension_id}/");
            manifest["allowed_origins"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == &origin))
        };
        Some(host.is_file() && caller)
    })();
    if valid == Some(true) {
        check(
            "ready",
            "Native browser registration matches Serein's setup receipt.",
            "",
        )
    } else {
        check(
            "blocked",
            "Native browser registration is missing or changed.",
            "Copy a fresh link instruction from Connections and run it in the local assistant.",
        )
    }
}

fn vault_policy(root: &Path, connection: &install::Connection) -> (Value, Option<Policy>) {
    let path = install::db_path(root, connection);
    if !path.is_file() {
        return (
            check(
                "blocked",
                "The linked local vault is missing.",
                "Open Connections and relink Serein. Existing data is not restored by relinking.",
            ),
            None,
        );
    }
    let policy = (|| -> Option<Policy> {
        let conn =
            SqliteConnection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let raw: String = conn
            .query_row(
                "SELECT policy FROM sources WHERE id=?1",
                [&connection.source_id],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str(&raw).ok()
    })();
    match policy {
        Some(policy) => (
            check("ready", "The linked vault is readable.", ""),
            Some(policy),
        ),
        None => (
            check(
                "blocked",
                "The linked vault cannot be read or its policy is invalid.",
                "Check local file permissions or restore a vault backup.",
            ),
            None,
        ),
    }
}

pub fn run(root: &Path) -> Result<Value> {
    let registry = install::read_registry(root)?;
    let connection = registry.default_vault.as_ref().and_then(|vault_id| {
        registry
            .connections
            .iter()
            .find(|c| c.vault_id == *vault_id)
    });
    let paired = connection.is_some_and(|c| c.paired);
    let browser = if paired {
        check(
            "paired",
            "This browser completed pairing previously. Live extension state is not checked here.",
            "",
        )
    } else {
        check(
            "blocked",
            "No default browser connection is paired.",
            "Open Connections in the extension and complete linking.",
        )
    };
    let native = connection.filter(|c| c.paired).map_or_else(
        || {
            check(
                "blocked",
                "There is no paired browser to check.",
                "Complete linking in the extension.",
            )
        },
        |c| native_host(root, &c.browser, &c.extension_id),
    );
    let (vault, policy) = connection.filter(|c| c.paired).map_or_else(
        || {
            (
                check(
                    "blocked",
                    "There is no paired vault to check.",
                    "Complete linking in the extension.",
                ),
                None,
            )
        },
        |c| vault_policy(root, c),
    );
    let model_available = model::Encoder::open(&root.join("models/current")).is_ok();
    let model = if model_available {
        check("ready", "The local semantic model opens successfully.", "")
    } else {
        check(
            "fallback",
            "The semantic model is unavailable; lexical recall remains available.",
            "Run a fresh link instruction from the installed skill to restore the model.",
        )
    };
    let adapter_list = install::inspect_adapters(root)?["adapters"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let selected = connection.map(|c| c.adapters.as_slice()).unwrap_or(&[]);
    let detected = adapter_list.iter().any(|adapter| {
        adapter["installed"] == true
            && selected
                .iter()
                .any(|id| adapter["id"].as_str() == Some(id.as_str()))
    });
    let generic_only = !selected.is_empty() && selected.iter().all(|id| id == "generic");
    let skill = if generic_only {
        check(
            "not_applicable",
            "The generic local executor has no standard skill location to inspect.",
            "",
        )
    } else if detected {
        check("detected", "A selected assistant's skill files were found. Assistant execution is not checked here.", "")
    } else {
        check(
            "unknown",
            "No selected assistant skill was detected in the usual locations.",
            "Install the GitHub skill in your local assistant, then link it from Connections.",
        )
    };
    let recall = match policy {
        Some(policy) if policy.consent && policy.recall_enabled => check(
            "ready",
            "Local recall is enabled. This check does not invoke an assistant.",
            "",
        ),
        Some(_) => check(
            "off",
            "Assistant recall is turned off in Privacy.",
            "Turn on assistant recall only if you want the paired assistant to use saved context.",
        ),
        None => check(
            "blocked",
            "Recall cannot use an unreadable or unpaired vault.",
            "Resolve the browser and vault checks above.",
        ),
    };
    let blocked = [&browser, &native, &vault, &recall]
        .iter()
        .any(|item| item["state"] == "blocked");
    let partial = !model_available || (!detected && !generic_only) || recall["state"] == "off";
    Ok(json!({
        "status": if blocked { "blocked" } else if partial { "partial" } else { "ok" },
        "version": env!("CARGO_PKG_VERSION"),
        "connection_count": registry.connections.len(),
        "model_available": model_available,
        "resident_process": false,
        "checks": {"browser":browser,"native_host":native,"vault":vault,"model":model,"skill":skill,"recall":recall},
    }))
}
