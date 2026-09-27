use crate::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub struct Vault {
    pub conn: Connection,
    pub path: PathBuf,
}
impl Vault {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            install::private_dir(p)?
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_millis(1500))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-4096; PRAGMA wal_autocheckpoint=256; PRAGMA secure_delete=ON;")?;
        let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver > 1 {
            return Err(Error(
                "SCHEMA_TOO_NEW",
                "Update Serein before opening this vault.".into(),
            ));
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT OR IGNORE INTO meta VALUES('evidence','0'),('privacy','0');
CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY,policy TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS site_rules(source TEXT NOT NULL,site TEXT NOT NULL,epoch INTEGER NOT NULL,excluded INTEGER NOT NULL,PRIMARY KEY(source,site));
CREATE TABLE IF NOT EXISTS atoms(id TEXT PRIMARY KEY,source TEXT NOT NULL,site TEXT NOT NULL,title TEXT NOT NULL,query TEXT,kind TEXT NOT NULL,first_seen TEXT NOT NULL,last_seen TEXT NOT NULL,seconds INTEGER NOT NULL,canonical TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS events(source TEXT NOT NULL,id TEXT NOT NULL,atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,visit TEXT NOT NULL,time TEXT NOT NULL,PRIMARY KEY(source,id));
CREATE TABLE IF NOT EXISTS atom_days(atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,day TEXT NOT NULL,session TEXT NOT NULL,mass REAL NOT NULL,PRIMARY KEY(atom,day,session));
CREATE VIRTUAL TABLE IF NOT EXISTS atom_fts USING fts5(id UNINDEXED,title,query,tokenize='unicode61');
CREATE TRIGGER IF NOT EXISTS atom_insert AFTER INSERT ON atoms BEGIN INSERT INTO atom_fts(id,title,query) VALUES(new.id,new.title,new.query); END;
CREATE TRIGGER IF NOT EXISTS atom_delete AFTER DELETE ON atoms BEGIN DELETE FROM atom_fts WHERE id=old.id; END;
CREATE TABLE IF NOT EXISTS vectors(atom TEXT PRIMARY KEY REFERENCES atoms(id) ON DELETE CASCADE,model TEXT NOT NULL,vector BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS feedback(id TEXT PRIMARY KEY,atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,action TEXT NOT NULL,text TEXT,time TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS receipts(id TEXT PRIMARY KEY,ids TEXT NOT NULL,evidence INTEGER NOT NULL,privacy INTEGER NOT NULL,bytes INTEGER NOT NULL,time TEXT NOT NULL,status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS topics(id TEXT PRIMARY KEY,label TEXT NOT NULL,centroid BLOB NOT NULL,model TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS atom_topics(atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,topic TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,mass REAL NOT NULL,PRIMARY KEY(atom,topic));
CREATE TRIGGER IF NOT EXISTS topic_invalidate AFTER DELETE ON atoms BEGIN DELETE FROM topics; END;
PRAGMA user_version=1;")?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key,value) VALUES('canonical_salt',?)",
            [id()],
        )?;
        install::private_file(path)?;
        Ok(Self {
            conn,
            path: path.into(),
        })
    }
    pub fn generation(&self) -> Result<(i64, i64)> {
        Ok((
            self.conn
                .query_row("SELECT value FROM meta WHERE key='evidence'", [], |r| {
                    r.get::<_, String>(0)
                })?
                .parse()
                .unwrap_or(0),
            self.conn
                .query_row("SELECT value FROM meta WHERE key='privacy'", [], |r| {
                    r.get::<_, String>(0)
                })?
                .parse()
                .unwrap_or(0),
        ))
    }
    pub fn policy(&self, source: &str) -> Result<Policy> {
        let s: Option<String> = self
            .conn
            .query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                r.get(0)
            })
            .optional()?;
        s.map(|x| serde_json::from_str(&x).map_err(Into::into))
            .unwrap_or(Ok(Policy::default()))
    }
    pub fn set_policy(&mut self, source: &str, p: &Policy) -> Result<Value> {
        if p.selected_sites.len() + p.excluded_sites.len() > 500
            || p.selected_sites
                .iter()
                .chain(p.excluded_sites.iter())
                .any(|x| !policy::valid_site(x))
        {
            return Err(invalid("Invalid site policy."));
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stored: Option<String> = tx
            .query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                r.get(0)
            })
            .optional()?;
        let old: Policy = stored
            .map(|x| serde_json::from_str(&x))
            .transpose()?
            .unwrap_or_default();
        if p.capture_epoch == old.capture_epoch
            && serde_json::to_value(p)? == serde_json::to_value(&old)?
        {
            drop(tx);
            return self.status(source);
        }
        if p.capture_epoch <= old.capture_epoch {
            return Err(invalid("Policy epoch must increase."));
        }
        tx.execute("INSERT INTO sources(id,policy) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET policy=excluded.policy",params![source,serde_json::to_string(p)?])?;
        tx.execute("UPDATE site_rules SET excluded=0 WHERE source=?", [source])?;
        for site in &p.excluded_sites {
            tx.execute("INSERT INTO site_rules VALUES(?,?,?,1) ON CONFLICT(source,site) DO UPDATE SET epoch=max(epoch,excluded.epoch),excluded=1",params![source,site,p.capture_epoch])?;
        }
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='privacy'",
            [],
        )?;
        tx.commit()?;
        self.status(source)
    }
    pub fn status(&self, source: &str) -> Result<Value> {
        let (e, p) = self.generation()?;
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))?;
        let pending: i64 = self.conn.query_row(
            "SELECT count(*) FROM atoms WHERE id NOT IN (SELECT atom FROM vectors)",
            [],
            |r| r.get(0),
        )?;
        let mut bytes = 0;
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        }
        Ok(
            json!({"protocol":1,"status":"ok","database_path":self.path,"evidence_generation":e,"privacy_generation":p,"atoms":count,"pending_atoms":pending,"vault_bytes":bytes,"policy":self.policy(source)?,"index_mode":if root().join("models/current/manifest.json").is_file(){"hybrid"}else{"lexical"},"model_available":root().join("models/current/manifest.json").is_file()}),
        )
    }
    pub fn ingest(&mut self, source: &str, epoch: u64, events: Vec<Event>) -> Result<Value> {
        if events.len() > 32 || serde_json::to_vec(&events)?.len() > 65536 {
            return Err(invalid("Batch exceeds its limit."));
        }
        for e in &events {
            e.validate()?
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let s: Option<String> = tx
            .query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                r.get(0)
            })
            .optional()?;
        let p: Policy = s
            .map(|x| serde_json::from_str(&x))
            .transpose()?
            .unwrap_or_default();
        let salt: String = tx.query_row(
            "SELECT value FROM meta WHERE key='canonical_salt'",
            [],
            |r| r.get(0),
        )?;
        let mut ack = vec![];
        let mut duplicate = vec![];
        let mut rejected = vec![];
        let mut added = 0;
        for raw in events {
            let eid = raw.event_id.clone();
            let reject = if epoch != p.capture_epoch {
                Some("STALE_CAPTURE_EPOCH")
            } else {
                None
            };
            if let Some(reason) = reject {
                rejected.push(json!({"id":eid,"reason":reason}));
                continue;
            }
            let rules = tx
                .prepare("SELECT site,epoch,excluded FROM site_rules WHERE source=?")?
                .query_map([source], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let relevant: Vec<_> = rules
                .iter()
                .filter(|(s, _, _)| policy::matches(&raw.site_key, s))
                .collect();
            let required = relevant.iter().map(|(_, e, _)| *e).max().unwrap_or(0);
            if raw.site_epoch < required || relevant.iter().any(|(_, _, x)| *x) {
                rejected.push(json!({"id":eid,"reason":"STALE_SITE_EPOCH"}));
                continue;
            }
            let e = match policy::allowed(&raw, &p) {
                Ok(x) => x,
                Err(reason) => {
                    rejected.push(json!({"id":eid,"reason":reason}));
                    continue;
                }
            };
            if tx
                .query_row(
                    "SELECT 1 FROM events WHERE source=? AND id=?",
                    params![source, eid],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                duplicate.push(eid);
                continue;
            }
            let canonical = hash(
                format!(
                    "{}\0{}\0{}\0{}\0{}\0{}",
                    salt,
                    source,
                    e.site_key,
                    e.kind,
                    e.title,
                    e.search_query.as_deref().unwrap_or("")
                )
                .as_bytes(),
            );
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM atoms WHERE canonical=?",
                    [&canonical],
                    |r| r.get(0),
                )
                .optional()?;
            let atom = existing.clone().unwrap_or_else(id);
            let count: i64 = tx.query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))?;
            let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
                + std::fs::metadata(format!("{}-wal", self.path.display()))
                    .map(|m| m.len())
                    .unwrap_or(0);
            if existing.is_none() && (count >= 10000 || bytes >= 96 * 1024 * 1024) {
                rejected.push(json!({"id":eid,"reason":"STORAGE_FULL"}));
                continue;
            }
            tx.execute("INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(canonical) DO UPDATE SET last_seen=max(last_seen,excluded.last_seen),seconds=min(3600,seconds+excluded.seconds)",params![atom,source,e.site_key,e.title,e.search_query,e.kind,e.observed_at,e.observed_at,e.foreground_seconds,canonical])?;
            tx.execute(
                "INSERT INTO events VALUES(?,?,?,?,?)",
                params![source, eid, atom, e.visit_id, e.observed_at],
            )?;
            let dt = chrono::DateTime::parse_from_rfc3339(&e.observed_at).unwrap();
            let day = dt.format("%Y-%m-%d").to_string();
            let session = format!("{}:{}:{}", source, epoch, dt.timestamp() / 1800);
            let mass = if e.kind == "search" {
                1.0
            } else {
                (0.25 + 0.5 * e.foreground_seconds as f64 / 60.0).min(0.75)
            };
            tx.execute("INSERT INTO atom_days VALUES(?,?,?,?) ON CONFLICT(atom,day,session) DO UPDATE SET mass=max(mass,excluded.mass)",params![atom,day,session,mass])?;
            ack.push(eid);
            added += 1;
        }
        if added > 0 {
            tx.execute(
                "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='evidence'",
                [],
            )?;
        }
        tx.execute("DELETE FROM atoms WHERE last_seen < strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') AND id NOT IN (SELECT atom FROM feedback WHERE action='confirm_constraint')",[])?;
        tx.execute("DELETE FROM events WHERE time < strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days') OR rowid IN (SELECT rowid FROM events ORDER BY time DESC LIMIT -1 OFFSET 20000)",[])?;
        tx.execute("DELETE FROM atom_days WHERE rowid IN (SELECT rowid FROM atom_days ORDER BY day DESC LIMIT -1 OFFSET 30000)",[])?;
        tx.commit()?;
        let mut r = self.status(source)?;
        r["acknowledged_ids"] = json!(ack);
        r["duplicate_ids"] = json!(duplicate);
        r["rejected"] = json!(rejected);
        Ok(r)
    }
    pub fn forget(
        &mut self,
        source: &str,
        site: Option<&str>,
        atom: Option<&str>,
        epoch: u64,
    ) -> Result<Value> {
        if let Some(s) = site {
            if !policy::valid_site(s) {
                return Err(invalid("Invalid hostname."));
            }
        }
        if let Some(a) = atom {
            check_id(a)?
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut count = 0;
        if let Some(site) = site {
            let old: u64 = tx
                .query_row(
                    "SELECT epoch FROM site_rules WHERE source=? AND site=?",
                    params![source, site],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            if epoch < old {
                return Ok(json!({"status":"ok","superseded":true,"deleted":0}));
            }
            tx.execute("INSERT INTO site_rules VALUES(?,?,?,1) ON CONFLICT(source,site) DO UPDATE SET epoch=max(epoch,excluded.epoch),excluded=1",params![source,site,epoch])?;
            count = tx.execute(
                "DELETE FROM atoms WHERE source=? AND (site=? OR site LIKE ?)",
                params![source, site, format!("%.{site}")],
            )?
        } else if let Some(a) = atom {
            count = tx.execute(
                "DELETE FROM atoms WHERE id=? AND source=?",
                params![a, source],
            )?
        } else {
            count += tx.execute("DELETE FROM atoms WHERE source=?", [source])?;
            let s: String =
                tx.query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                    r.get(0)
                })?;
            let mut p: Policy = serde_json::from_str(&s)?;
            if !p.paused {
                p.capture_epoch = p.capture_epoch.max(epoch) + 1;
            }
            p.paused = true;
            tx.execute(
                "UPDATE sources SET policy=? WHERE id=?",
                params![serde_json::to_string(&p)?, source],
            )?;
        }
        tx.execute("DELETE FROM receipts", [])?;
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key IN ('evidence','privacy')",
            [],
        )?;
        tx.commit()?;
        Ok(
            json!({"status":"ok","deleted":count,"privacy_generation":self.generation()?.1,"logical_deletion":true}),
        )
    }
    pub fn feedback(
        &mut self,
        source: &str,
        atom: &str,
        action: &str,
        text: Option<&str>,
    ) -> Result<Value> {
        check_id(atom)?;
        if ![
            "not_about_me",
            "wrong_topic",
            "temporary_research",
            "confirm_constraint",
            "do_not_use",
        ]
        .contains(&action)
        {
            return Err(invalid("Unknown correction."));
        }
        if action == "confirm_constraint"
            && (text.is_none()
                || text.unwrap().trim().is_empty()
                || text.unwrap().chars().count() > 512
                || policy::sensitive(text.unwrap()))
        {
            return Err(invalid("Provide a non-sensitive explicit constraint."));
        }
        if self
            .conn
            .query_row(
                "SELECT 1 FROM atoms WHERE id=? AND source=?",
                params![atom, source],
                |_| Ok(()),
            )
            .optional()?
            .is_none()
        {
            return Err(invalid("Evidence not found."));
        }
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM feedback WHERE action='confirm_constraint'",
            [],
            |r| r.get(0),
        )?;
        if action == "confirm_constraint" && count >= 1000 {
            return Err(Error(
                "STORAGE_FULL",
                "Remove an existing confirmed constraint first.".into(),
            ));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO feedback VALUES(?,?,?,?,?)",
            params![
                id(),
                atom,
                action,
                text.map(|x| policy::clean(x, 512)),
                now()
            ],
        )?;
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='privacy'",
            [],
        )?;
        tx.commit()?;
        Ok(json!({"status":"ok"}))
    }
    pub fn dashboard(&self, source: &str) -> Result<Value> {
        let mut cards = vec![];
        let mut stmt=self.conn.prepare("SELECT a.id,a.site,a.title,a.query,a.last_seen,(SELECT count(DISTINCT session) FROM atom_days WHERE atom=a.id) FROM atoms a WHERE source=? AND (last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') OR id IN (SELECT atom FROM feedback WHERE action='confirm_constraint')) ORDER BY last_seen DESC LIMIT 60")?;
        let rows = stmt.query_map([source], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (id, site, title, query, time, sessions) = row?;
            let corrections = self.corrections(&id)?;
            let confirmed = corrections
                .iter()
                .rev()
                .find(|x| x["action"] == "confirm_constraint");
            cards.push(json!({"id":id,"site":site,"text":confirmed.map(|x|x["text"].clone()).unwrap_or(json!(query.unwrap_or(title))),"state":if confirmed.is_some(){"confirmed"}else{"observed"},"last_seen":time,"sessions":sessions,"sites":1,"corrections":corrections}));
        }
        let mut r = self.status(source)?;
        r["cards"] = json!(cards);
        r["topics"] = inference::activity(&self.conn)?;
        Ok(r)
    }
    fn corrections(&self, id: &str) -> Result<Vec<Value>> {
        Ok(self
            .conn
            .prepare("SELECT action,text FROM feedback WHERE atom=? ORDER BY time")?
            .query_map([id], |r| {
                Ok(json!({"action":r.get::<_,String>(0)?,"text":r.get::<_,Option<String>>(1)?}))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn refresh(&mut self, model_path: &Path, budget: u64) -> Result<Value> {
        let start = Instant::now();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path.with_extension("refresh.lock"))?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return Ok(json!({"status":"partial","reason":"REFRESH_BUSY"}));
        }
        let encoder = match model::Encoder::open(model_path) {
            Ok(m) => m,
            Err(_) => {
                return Ok(
                    json!({"status":"partial","mode":"lexical","reason":"MODEL_UNAVAILABLE"}),
                )
            }
        };
        let generation = self.generation()?;
        let rows=self.conn.prepare("SELECT id,title,query FROM atoms WHERE id NOT IN (SELECT atom FROM vectors WHERE model=?) OR id NOT IN (SELECT atom FROM atom_topics) LIMIT 256")?.query_map([&encoder.manifest.model_hash],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut processed = 0;
        for (id, title, query) in rows {
            if start.elapsed().as_millis() as u64 >= budget {
                break;
            }
            let tv = encoder.encode(&title);
            let qv = query.as_ref().and_then(|x| encoder.encode(x));
            let v = match (tv, qv) {
                (Some(t), Some(q)) => {
                    model::normalize(t.iter().zip(q).map(|(t, q)| 0.3 * t + 0.7 * q).collect())
                }
                (t, q) => t.or(q),
            };
            if let Some(v) = v {
                let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
                let tx = self.conn.transaction()?;
                let current: i64 = tx
                    .query_row("SELECT value FROM meta WHERE key='privacy'", [], |r| {
                        r.get::<_, String>(0)
                    })?
                    .parse()
                    .unwrap_or(-1);
                if current != generation.1 {
                    break;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO vectors VALUES(?,?,?)",
                    params![id, encoder.manifest.model_hash, bytes],
                )?;
                inference::assign(
                    &tx,
                    &id,
                    &v,
                    query.as_deref().unwrap_or(&title),
                    &encoder.manifest.model_hash,
                )?;
                tx.commit()?;
                processed += 1
            }
        }
        let pending: i64 = self.conn.query_row(
            "SELECT count(*) FROM atoms WHERE id NOT IN (SELECT atom FROM vectors WHERE model=?)",
            [&encoder.manifest.model_hash],
            |r| r.get(0),
        )?;
        Ok(
            json!({"status":if pending>0{"partial"}else{"ok"},"processed":processed,"pending_atoms":pending,"mode":"hybrid"}),
        )
    }
    pub fn recall(&mut self, r: &Recall, source: &str, model_path: &Path) -> Result<Value> {
        r.validate()?;
        let start = Instant::now();
        let budget = r.budget_ms.clamp(250, 2000);
        let p = self.policy(source)?;
        if !p.recall_enabled || !p.consent {
            return Ok(
                json!({"protocol":1,"request_id":r.request_id,"status":"blocked","context":[],"warnings":["Assistant recall is disabled in Privacy."]}),
            );
        }
        let refresh = self.refresh(model_path, budget.saturating_sub(150))?;
        let encoder = if start.elapsed().as_millis() < (budget / 2) as u128 {
            model::Encoder::open(model_path).ok()
        } else {
            None
        };
        let generation = self.generation()?;
        let terms: Vec<String> = std::iter::once(&r.query)
            .chain(r.facets.iter())
            .map(|x| x.to_lowercase())
            .collect();
        let identifier_pattern = regex::Regex::new(
            r"(?i)\b(?:[a-z]+[0-9][a-z0-9-]*|[0-9]+(?:[.,][0-9]+)?\s*(?:cm|mm|kg|gb|tb))\b",
        )
        .unwrap();
        let exact_requirements: Vec<String> = terms
            .iter()
            .flat_map(|t| {
                identifier_pattern
                    .find_iter(t)
                    .map(|m| m.as_str().split_whitespace().collect::<String>())
                    .collect::<Vec<_>>()
            })
            .collect();
        let tokens: Vec<String> = terms
            .iter()
            .flat_map(|s| {
                s.split(|c: char| !c.is_alphanumeric())
                    .filter(|x| x.chars().count() > 2)
                    .map(String::from)
            })
            .filter(|s| {
                ![
                    "what",
                    "about",
                    "the",
                    "and",
                    "with",
                    "that",
                    "this",
                    "does",
                    "have",
                    "for",
                    "are",
                    "was",
                    "can",
                    "how",
                    "why",
                    "which",
                    "when",
                    "where",
                    "who",
                    "you",
                    "your",
                    "our",
                    "its",
                    "has",
                    "had",
                    "did",
                    "could",
                    "would",
                    "should",
                    "from",
                    "into",
                    "than",
                    "then",
                    "not",
                    "but",
                    "without",
                    "best",
                    "topic",
                    "intent",
                    "constraint",
                    "recall",
                    "der",
                    "die",
                    "das",
                    "ein",
                    "eine",
                    "einer",
                    "eines",
                    "für",
                    "dem",
                    "den",
                    "mit",
                    "von",
                    "zum",
                    "zur",
                    "auf",
                    "aus",
                    "als",
                    "welche",
                    "welcher",
                    "welches",
                    "wie",
                    "hatte",
                    "ich",
                    "mein",
                    "meine",
                    "voor",
                    "het",
                    "een",
                    "dat",
                    "wat",
                    "welk",
                    "had",
                    "mijn",
                    "met",
                    "van",
                    "naar",
                    "welke",
                    "pour",
                    "les",
                    "des",
                    "une",
                    "que",
                    "quel",
                    "quelle",
                    "quelles",
                    "quels",
                    "dans",
                    "avec",
                    "sans",
                    "sur",
                    "mon",
                    "mes",
                    "aux",
                    "est",
                    "ces",
                    "cette",
                    "comment",
                    "avais",
                ]
                .contains(&s.as_str())
            })
            .collect();
        let fts_query = tokens
            .iter()
            .take(32)
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let lexical_ids: HashSet<String> = if fts_query.is_empty() {
            HashSet::new()
        } else {
            self.conn.prepare("SELECT id FROM atom_fts WHERE atom_fts MATCH ? ORDER BY bm25(atom_fts) LIMIT 40")?.query_map([fts_query],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?
        };
        let qvectors: Vec<Vec<f32>> = encoder
            .as_ref()
            .map(|e| terms.iter().filter_map(|t| e.encode(t)).collect())
            .unwrap_or_default();
        let mut candidates = vec![];
        let mut stmt=self.conn.prepare("SELECT id,site,title,query,last_seen FROM atoms WHERE source=? AND (last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') OR id IN (SELECT atom FROM feedback WHERE action='confirm_constraint')) ORDER BY last_seen DESC LIMIT 10000")?;
        let rows = stmt.query_map([source], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            if start.elapsed().as_millis() as u64 >= budget {
                break;
            }
            let (id, site, title, query, time) = row?;
            if p.excluded_sites.iter().any(|x| policy::matches(&site, x))
                || p.selected_only && !p.selected_sites.iter().any(|x| policy::matches(&site, x))
            {
                continue;
            }
            let corrections = self.corrections(&id)?;
            if corrections
                .iter()
                .any(|c| c["action"] == "do_not_use" || c["action"] == "wrong_topic")
            {
                continue;
            }
            let confirmed = corrections
                .iter()
                .rev()
                .find(|c| c["action"] == "confirm_constraint");
            if confirmed.is_some() && !r.scope.contains(&"confirmed_preferences".into())
                || confirmed.is_none()
                    && !r.scope.iter().any(|x| x == "research" || x == "projects")
            {
                continue;
            }
            let raw = confirmed
                .and_then(|c| c["text"].as_str())
                .unwrap_or(query.as_deref().unwrap_or(&title));
            let lower = raw.to_lowercase();
            if !exact_requirements.is_empty() {
                let candidate_identifiers: HashSet<String> = identifier_pattern
                    .find_iter(&lower)
                    .map(|m| m.as_str().split_whitespace().collect::<String>())
                    .collect();
                if !exact_requirements
                    .iter()
                    .any(|x| candidate_identifiers.contains(x))
                {
                    continue;
                }
            }

            let words: HashSet<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
            let lexical = tokens
                .iter()
                .filter(|t| {
                    if t.is_ascii() {
                        words.contains(t.as_str())
                    } else {
                        lower.contains(t.as_str())
                    }
                })
                .count() as f32;
            let lexical = if !exact_requirements.is_empty() {
                lexical.max(1.0)
            } else if lexical_ids.contains(&id) {
                lexical.max(1.0)
            } else if confirmed.is_some() || tokens.iter().any(|t| !t.is_ascii()) {
                lexical
            } else {
                0.0
            };
            let mut semantic = 0f32;
            if let Some(enc) = &encoder {
                let vector: Option<Vec<u8>> = self
                    .conn
                    .query_row(
                        "SELECT vector FROM vectors WHERE atom=? AND model=?",
                        params![id, enc.manifest.model_hash],
                        |x| x.get(0),
                    )
                    .optional()?;
                if let Some(v) = vector {
                    let v: Vec<f32> = v
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                        .collect();
                    semantic = qvectors
                        .iter()
                        .map(|q| model::cosine(q, &v))
                        .fold(0f32, f32::max)
                }
            }
            if lexical == 0.0 && semantic < 0.55 {
                continue;
            }
            let sessions: i64 = self.conn.query_row(
                "SELECT count(DISTINCT session) FROM atom_days WHERE atom=?",
                [&id],
                |x| x.get(0),
            )?;
            let mut limits = vec![
                "Browsing does not establish endorsement, ownership, or a settled preference."
                    .to_string(),
            ];
            for c in &corrections {
                if let Some(a) = c["action"].as_str() {
                    if a != "confirm_constraint" {
                        limits.push(format!("User correction: {}", a.replace('_', " ")))
                    }
                }
            }
            let subject = if corrections.iter().any(|c| c["action"] == "not_about_me") {
                "other"
            } else if confirmed.is_some() {
                "self"
            } else {
                "unknown"
            };
            let text = if confirmed.is_some() {
                raw.to_owned()
            } else {
                format!(
                    "Observed {} on {}: {}",
                    if query.is_some() {
                        "search"
                    } else {
                        "page title"
                    },
                    site,
                    raw
                )
            };
            candidates.push((lexical,semantic,site.clone(),json!({"id":id,"kind":if confirmed.is_some(){"constraint"}else{"research_topic"},"state":if confirmed.is_some(){"confirmed"}else{"observed"},"text":text,"subject":subject,"evidence":{"sessions":sessions,"sites":1},"last_seen":time,"limits":limits})));
            if candidates.len() > 96 {
                candidates.sort_by(|a, b| (b.0 + b.1).total_cmp(&(a.0 + a.1)));
                candidates.truncate(96)
            }
        }
        drop(stmt);
        // Reciprocal-rank fusion keeps incomparable lexical/semantic scales separate.
        let mut ranks: HashMap<String, f32> = HashMap::new();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (rank, c) in candidates.iter().filter(|c| c.0 > 0.0).take(40).enumerate() {
            *ranks
                .entry(c.3["id"].as_str().unwrap().to_string())
                .or_default() += 1.0 / (61 + rank) as f32;
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (rank, c) in candidates
            .iter()
            .filter(|c| c.1 >= 0.55)
            .take(40)
            .enumerate()
        {
            *ranks
                .entry(c.3["id"].as_str().unwrap().to_string())
                .or_default() += 1.0 / (61 + rank) as f32;
        }
        let mut candidates: Vec<_> = candidates
            .into_iter()
            .filter_map(|(_, _, site, record)| {
                ranks
                    .get(record["id"].as_str().unwrap())
                    .map(|score| (*score, site, record))
            })
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        candidates.truncate(64);
        let mut packet = json!({"protocol":1,"request_id":r.request_id,"status":"partial","as_of":now(),"generation":{"evidence":generation.0,"privacy":generation.1},"index":{"mode":if encoder.is_some(){"hybrid"}else{"lexical"},"pending_atoms":self.status(source)?["pending_atoms"]},"context":[],"alternatives":[],"warnings":if encoder.is_none(){vec!["Semantic model unavailable; lexical matching only."]}else{vec![]}});
        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut seen = HashSet::new();
        for (_, site, record) in candidates {
            if packet["context"].as_array().unwrap().len() >= 6 {
                break;
            }
            if *counts.get(&site).unwrap_or(&0) >= 2 || !seen.insert(record["text"].to_string()) {
                continue;
            }
            packet["context"].as_array_mut().unwrap().push(record);
            if serde_json::to_vec(&packet)?.len() + 64 > r.max_bytes {
                packet["context"].as_array_mut().unwrap().pop();
            } else {
                *counts.entry(site).or_default() += 1
            }
        }
        let empty = packet["context"].as_array().unwrap().is_empty();
        packet["status"] = json!(if empty {
            "empty"
        } else if encoder.is_none()
            || refresh["status"] != "ok"
            || start.elapsed().as_millis() as u64 >= budget
        {
            "partial"
        } else {
            "ok"
        });
        let receipt_tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current_privacy: i64 = receipt_tx.query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='privacy'",
            [],
            |r| r.get(0),
        )?;
        if current_privacy != generation.1 {
            packet["context"] = json!([]);
            packet["status"] = json!("blocked");
            packet["warnings"] = json!(["Privacy changed during recall. Retry."])
        }
        let ids: Vec<Value> = packet["context"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["id"].clone())
            .collect();
        let size = serde_json::to_vec(&packet)?.len();
        if size > r.max_bytes {
            packet = json!({"protocol":1,"request_id":r.request_id,"status":"empty","context":[],"warnings":["Packet budget too small."]})
        }
        receipt_tx.execute(
            "INSERT OR REPLACE INTO receipts VALUES(?,?,?,?,?,?,?)",
            params![
                r.request_id,
                serde_json::to_string(&ids)?,
                generation.0,
                generation.1,
                serde_json::to_vec(&packet)?.len(),
                now(),
                packet["status"].as_str()
            ],
        )?;
        receipt_tx.execute("DELETE FROM receipts WHERE time<strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days') OR rowid IN (SELECT rowid FROM receipts ORDER BY time DESC LIMIT -1 OFFSET 1000)",[])?;
        receipt_tx.commit()?;
        Ok(packet)
    }
    pub fn receipts(&self) -> Result<Value> {
        let rows=self.conn.prepare("SELECT id,ids,evidence,privacy,bytes,time,status FROM receipts ORDER BY time DESC LIMIT 100")?.query_map([],|r|Ok(json!({"request_id":r.get::<_,String>(0)?,"ids":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(json!([])),"evidence_generation":r.get::<_,i64>(2)?,"privacy_generation":r.get::<_,i64>(3)?,"bytes":r.get::<_,i64>(4)?,"time":r.get::<_,String>(5)?,"status":r.get::<_,String>(6)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(json!({"status":"ok","receipts":rows}))
    }
}

impl Vault {
    pub fn explain(&self, source: &str, ids: &[String], max_bytes: usize) -> Result<Value> {
        if ids.len() > 6 || !(512..=4096).contains(&max_bytes) {
            return Err(invalid(
                "Explain accepts at most six evidence IDs and 512–4096 bytes.",
            ));
        }
        let p = self.policy(source)?;
        if !p.recall_enabled || !p.consent {
            return Ok(json!({"status":"blocked","evidence":[]}));
        }
        let generation = self.generation()?;
        let mut packet = json!({"protocol":1,"status":"ok","generation":{"evidence":generation.0,"privacy":generation.1},"evidence":[]});
        for id in ids {
            check_id(id)?;
            let row:Option<(String,String,Option<String>,String)>=self.conn.query_row("SELECT site,title,query,last_seen FROM atoms WHERE id=? AND source=? AND (last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') OR id IN (SELECT atom FROM feedback WHERE action='confirm_constraint'))",params![id,source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((site, title, query, time)) = row {
                if p.excluded_sites.iter().any(|r| policy::matches(&site, r))
                    || p.selected_only
                        && !p.selected_sites.iter().any(|r| policy::matches(&site, r))
                {
                    continue;
                }
                let corrections = self.corrections(id)?;
                if corrections.iter().any(|c| c["action"] == "do_not_use") {
                    continue;
                }
                packet["evidence"].as_array_mut().unwrap().push(json!({"id":id,"site":site,"title":title,"search_query":query,"last_seen":time,"corrections":corrections,"source_type":"untrusted_browser_metadata"}));
                if serde_json::to_vec(&packet)?.len() > max_bytes {
                    packet["evidence"].as_array_mut().unwrap().pop();
                    break;
                }
            }
        }
        if self.generation()?.1 != generation.1 {
            packet["evidence"] = json!([]);
            packet["status"] = json!("blocked")
        };
        Ok(packet)
    }
}
