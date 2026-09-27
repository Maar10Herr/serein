use serde_json::{json, Value};
use serein_core::*;
use std::io::Read;
fn input() -> Result<Value> {
    let mut b = vec![];
    std::io::stdin().take(16385).read_to_end(&mut b)?;
    if b.len() > 16384 {
        return Err(invalid("Request exceeds 16 KiB."));
    }
    if b.starts_with(&[239, 187, 191]) {
        b.drain(..3);
    }
    Ok(serde_json::from_slice(&b)?)
}
fn run() -> Result<Value> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    match args.first().map(String::as_str) {
        Some("--version") => Ok(json!({"version":env!("CARGO_PKG_VERSION")})),
        Some("setup") => install::setup(&root, serde_json::from_value(input()?)?),
        Some("doctor") => {
            let r = install::read_registry(&root)?;
            Ok(
                json!({"status":if r.connections.iter().any(|c|c.paired){"ok"}else{"blocked"},"version":env!("CARGO_PKG_VERSION"),"data_root":root,"executable":std::env::current_exe()?,"default_vault":r.default_vault,"connections":r.connections.iter().map(|c|json!({"vault_id":c.vault_id,"label":c.label,"paired":c.paired})).collect::<Vec<_>>(),"model_available":root.join("models/current/manifest.json").is_file(),"resident_process":false}),
            )
        }
        Some("recall") => {
            let r: Recall = serde_json::from_value(input()?)?;
            r.validate()?;
            let c = install::resolve(&root, &r.vault)?;
            let mut v = storage::Vault::open(&install::db_path(&root, &c))?;
            v.recall(&r, &c.source_id, &root.join("models/current"))
        }
        Some("explain") => {
            let req = input()?;
            let c = install::resolve(&root, req["vault"].as_str().unwrap_or("default"))?;
            let v = storage::Vault::open(&install::db_path(&root, &c))?;
            if !v.policy(&c.source_id)?.recall_enabled {
                return Ok(json!({"status":"blocked","context":[]}));
            }
            let ids: Vec<String> = serde_json::from_value(req["evidence_ids"].clone())
                .map_err(|_| invalid("Provide evidence_ids from a recall packet."))?;
            v.explain(
                &c.source_id,
                &ids,
                req["max_bytes"].as_u64().unwrap_or(4096) as usize,
            )
        }
        Some("refresh") => {
            let c = install::resolve(&root, "default")?;
            let mut v = storage::Vault::open(&install::db_path(&root, &c))?;
            let budget = args
                .iter()
                .position(|x| x == "--budget-ms")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse().ok())
                .unwrap_or(1500)
                .min(30000);
            v.refresh(&root.join("models/current"), budget)
        }
        Some("adapters") => match args.get(1).map(String::as_str) {
            Some("inspect") => install::inspect_adapters(&root),
            Some("install") => {
                let req = input()?;
                let names: Vec<String> = serde_json::from_value(req["adapters"].clone())?;
                let exe = std::env::current_exe()?.canonicalize()?;
                install::install_adapters(&root, &names, &exe)
            }
            Some("verify") => {
                let inspected = install::inspect_adapters(&root)?;
                Ok(
                    json!({"status":"partial","adapters":inspected["adapters"],"warning":"Filesystem presence is only a detection signal. Assistant discovery and skill-triggered execution must be tested inside each assistant."}),
                )
            }
            _ => Err(invalid("Use adapters inspect, install, or verify.")),
        },
        Some("export") => {
            let req = input()?;
            if req["consent"] != true {
                return Err(Error(
                    "ACCESS_DENIED",
                    "Explicit export consent required.".into(),
                ));
            }
            let c = install::resolve(&root, "default")?;
            let v = storage::Vault::open(&install::db_path(&root, &c))?;
            v.dashboard(&c.source_id)
        }
        Some("uninstall") => {
            let req = input()?;
            if req["consent"] != true {
                return Err(Error(
                    "ACCESS_DENIED",
                    "Explicit uninstall consent required.".into(),
                ));
            }
            install::uninstall(&root, req["erase_vaults"] == true)
        }
        _ => Err(invalid(
            "Commands: setup, doctor, recall, explain, refresh, adapters, export, uninstall.",
        )),
    }
}
fn main() {
    match run() {
        Ok(v) => {
            println!("{}", v)
        }
        Err(e) => {
            println!("{}", error_value(&e));
            std::process::exit(match e.0 {
                "INVALID_REQUEST" => 2,
                "DB_BUSY" => 4,
                "ACCESS_DENIED" | "NO_DEFAULT_VAULT" | "UNPAIRED_SOURCE" => 3,
                _ => 5,
            })
        }
    }
}
