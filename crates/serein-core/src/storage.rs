use crate::algorithm;
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
        if ver > 3 {
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
CREATE TABLE IF NOT EXISTS topic_skips(atom TEXT PRIMARY KEY REFERENCES atoms(id) ON DELETE CASCADE,model TEXT NOT NULL);
CREATE TRIGGER IF NOT EXISTS topic_deleted_reconsider_skips AFTER DELETE ON topics BEGIN DELETE FROM topic_skips WHERE model=old.model; END;
        ")?;
        if ver < 2 {
            conn.execute_batch("BEGIN IMMEDIATE;
DROP TRIGGER IF EXISTS topic_invalidate;
CREATE TRIGGER topic_invalidate BEFORE DELETE ON atoms BEGIN DELETE FROM topics WHERE id IN (SELECT topic FROM atom_topics WHERE atom=old.id); END;
PRAGMA user_version=3;
COMMIT;")?;
        } else if ver < 3 {
            conn.execute_batch("PRAGMA user_version=3;")?;
        }
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
    fn current_model_hash(&self) -> Option<String> {
        model::current_hash(&root().join("models/current"))
    }
    fn pending_atoms_for_model(&self, model_hash: Option<&str>) -> Result<i64> {
        match model_hash {
            Some(hash) => Ok(self.conn.query_row(
                "SELECT count(*) FROM atoms a
WHERE NOT EXISTS (SELECT 1 FROM vectors v WHERE v.atom=a.id AND v.model=?)
   OR (NOT EXISTS (SELECT 1 FROM atom_topics m JOIN topics t ON t.id=m.topic WHERE m.atom=a.id AND t.model=?)
       AND NOT EXISTS (SELECT 1 FROM topic_skips s WHERE s.atom=a.id AND s.model=?))",
                params![hash,hash,hash],
                |r| r.get(0),
            )?),
            None => Ok(self.conn.query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))?),
        }
    }
    fn unindexed_atoms(
        &self,
        model_hash: &str,
        limit: i64,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let rows = self.conn.prepare("SELECT a.id,a.title,a.query FROM atoms a
WHERE NOT EXISTS (SELECT 1 FROM vectors v WHERE v.atom=a.id AND v.model=?)
   OR (NOT EXISTS (SELECT 1 FROM atom_topics m JOIN topics t ON t.id=m.topic WHERE m.atom=a.id AND t.model=?)
       AND NOT EXISTS (SELECT 1 FROM topic_skips s WHERE s.atom=a.id AND s.model=?))
ORDER BY a.first_seen,a.id LIMIT ?")?
            .query_map(params![model_hash,model_hash,model_hash,limit],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(rows)
    }
    fn activate_model(&mut self, model_hash: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='active_model_hash'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if previous.as_deref() != Some(model_hash) {
            tx.execute("DELETE FROM topics", [])?;
            tx.execute("DELETE FROM topic_skips", [])?;
            tx.execute("DELETE FROM vectors WHERE model<>?", [model_hash])?;
            tx.execute("INSERT INTO meta(key,value) VALUES('active_model_hash',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [model_hash])?;
        }
        tx.commit()?;
        Ok(())
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
        let current_model = self.current_model_hash();
        let pending = self.pending_atoms_for_model(current_model.as_deref())?;
        let mut bytes = 0;
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        }
        Ok(
            json!({"protocol":1,"status":"ok","database_path":self.path,"evidence_generation":e,"privacy_generation":p,"atoms":count,"pending_atoms":pending,"vault_bytes":bytes,"policy":self.policy(source)?,"index_mode":if current_model.is_some(){"hybrid"}else{"lexical"},"model_available":current_model.is_some()}),
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
        r["topics"] = inference::activity(&self.conn, self.current_model_hash().as_deref())?;
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
        let encoder = match model::Encoder::open(model_path) {
            Ok(m) => m,
            Err(_) => {
                return Ok(
                    json!({"status":"partial","mode":"lexical","reason":"MODEL_UNAVAILABLE"}),
                )
            }
        };
        self.refresh_with_encoder(&encoder, budget)
    }
    fn refresh_with_encoder(&mut self, encoder: &model::Encoder, budget: u64) -> Result<Value> {
        let start = Instant::now();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path.with_extension("refresh.lock"))?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return Ok(json!({"status":"partial","reason":"REFRESH_BUSY"}));
        }
        self.activate_model(&encoder.manifest.model_hash)?;
        let generation = self.generation()?;
        let rows = self.unindexed_atoms(&encoder.manifest.model_hash, 256)?;
        let mut processed = 0;
        for (id, title, query) in rows {
            if start.elapsed().as_millis() as u64 >= budget {
                break;
            }
            let tv = encoder.encode(&title);
            let qv = query.as_ref().and_then(|x| encoder.encode(x));
            let v = match (tv, qv) {
                (Some(t), Some(q)) => model::normalize(
                    t.iter()
                        .zip(q)
                        .map(|(t, q)| algorithm::TITLE_WEIGHT * t + algorithm::QUERY_WEIGHT * q)
                        .collect(),
                ),
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
        let pending = self.pending_atoms_for_model(Some(&encoder.manifest.model_hash))?;
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
        let encoder = model::Encoder::open(model_path).ok();
        let remaining = budget.saturating_sub(start.elapsed().as_millis() as u64);
        let refresh_budget = (budget / 4).min(250).min(remaining / 2);
        let refresh = match encoder.as_ref() {
            Some(encoder) => self.refresh_with_encoder(encoder, refresh_budget)?,
            None => json!({"status":"partial","mode":"lexical","reason":"MODEL_UNAVAILABLE"}),
        };
        // If startup or refresh consumed the request window, report the
        // retrieval path actually used instead of claiming a hybrid search.
        let encoder =
            encoder.filter(|_| (start.elapsed().as_millis() as u64) < budget.saturating_sub(25));
        let generation = self.generation()?;
        let candidates = crate::retrieval::candidates(
            &self.conn,
            source,
            &p,
            r,
            encoder.as_ref(),
            start,
            budget,
        )?;
        let mut packet = json!({"protocol":1,"request_id":r.request_id,"status":"partial","as_of":now(),"generation":{"evidence":generation.0,"privacy":generation.1},"index":{"mode":if encoder.is_some(){"hybrid"}else{"lexical"},"pending_atoms":self.status(source)?["pending_atoms"]},"context":[],"alternatives":[],"warnings":if encoder.is_none(){vec!["Semantic model unavailable; lexical matching only."]}else{vec![]}});
        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut seen = HashSet::new();
        for (_, site, record) in candidates {
            if packet["context"].as_array().unwrap().len() >= algorithm::MAX_CONTEXT_RECORDS {
                break;
            }
            if *counts.get(&site).unwrap_or(&0) >= algorithm::MAX_RECORDS_PER_SITE
                || !seen.insert(record["text"].to_string())
            {
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(&dir.path().join("vault.sqlite")).unwrap();
        (dir, vault)
    }

    fn insert_atom(v: &Vault, atom: &str, first_seen: &str, vector: &[f32], model: &str) {
        v.conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    atom,
                    "source",
                    "example.com",
                    format!("Title {atom}"),
                    Option::<String>::None,
                    "visit",
                    first_seen,
                    now(),
                    30,
                    atom,
                ],
            )
            .unwrap();
        v.conn
            .execute(
                "INSERT INTO atom_days VALUES(?,?,?,?)",
                params![atom, chrono::Utc::now().date_naive().to_string(), atom, 1.0,],
            )
            .unwrap();
        let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
        v.conn
            .execute(
                "INSERT INTO vectors VALUES(?,?,?)",
                params![atom, model, bytes],
            )
            .unwrap();
    }

    fn axis(index: usize) -> Vec<f32> {
        let mut vector = vec![0.0; 256];
        vector[index] = 1.0;
        vector
    }

    #[test]
    fn status_pending_atoms_tracks_active_model() {
        let (_dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "old");
        inference::assign(&v.conn, "a", &axis(0), "Old", "old").unwrap();
        assert_eq!(v.pending_atoms_for_model(Some("old")).unwrap(), 0);
        assert_eq!(v.pending_atoms_for_model(Some("new")).unwrap(), 1);
    }

    #[test]
    fn model_change_leaves_no_old_model_topic_assignments() {
        let (_dir, mut v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "old");
        v.activate_model("old").unwrap();
        inference::assign(&v.conn, "a", &axis(0), "Old label", "old").unwrap();
        assert_eq!(
            inference::activity(&v.conn, Some("old"))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(inference::activity(&v.conn, Some("new"))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        v.activate_model("new").unwrap();
        for table in ["topics", "atom_topics", "vectors"] {
            let count: i64 = v
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        assert_eq!(v.pending_atoms_for_model(Some("new")).unwrap(), 1);
    }

    #[test]
    fn topic_rebuild_is_deterministic_given_same_atoms() {
        let (_dir, mut v) = vault();
        v.activate_model("model").unwrap();
        insert_atom(&v, "b", "2026-01-02T00:00:00Z", &axis(0), "model");
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(1), "model");
        let rebuild = |v: &Vault| {
            let ids = v.unindexed_atoms("model", 256).unwrap();
            assert_eq!(
                ids.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
                vec!["a", "b"]
            );
            for (atom, _, _) in ids {
                let vector = if atom == "a" { axis(1) } else { axis(0) };
                inference::assign(&v.conn, &atom, &vector, &format!("Title {atom}"), "model")
                    .unwrap();
            }
            v.conn.prepare("SELECT m.atom,m.topic,t.label FROM atom_topics m JOIN topics t ON t.id=m.topic ORDER BY m.atom,m.topic").unwrap()
                .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap()
                .collect::<std::result::Result<Vec<_>,_>>().unwrap()
        };
        let first = rebuild(&v);
        v.conn.execute("DELETE FROM topics", []).unwrap();
        assert_eq!(first, rebuild(&v));
    }

    #[test]
    fn reassignment_does_not_duplicate_old_and_new_topic_membership() {
        let (_dir, mut v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "old");
        v.activate_model("old").unwrap();
        inference::assign(&v.conn, "a", &axis(0), "Old", "old").unwrap();
        v.activate_model("new").unwrap();
        v.conn
            .execute(
                "INSERT INTO vectors VALUES(?,?,?)",
                params![
                    "a",
                    "new",
                    axis(1)
                        .iter()
                        .flat_map(|x| x.to_le_bytes())
                        .collect::<Vec<_>>()
                ],
            )
            .unwrap();
        inference::assign(&v.conn, "a", &axis(1), "New", "new").unwrap();
        inference::assign(&v.conn, "a", &axis(1), "New", "new").unwrap();
        let count: i64 = v
            .conn
            .query_row("SELECT count(*) FROM atom_topics WHERE atom='a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        let wrong:i64=v.conn.query_row("SELECT count(*) FROM atom_topics m JOIN topics t ON t.id=m.topic WHERE t.model<>'new'",[],|r|r.get(0)).unwrap();
        assert_eq!(wrong, 0);
    }

    #[test]
    fn deleting_one_atom_preserves_unrelated_topics() {
        let (_dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "model");
        insert_atom(&v, "b", "2026-01-02T00:00:00Z", &axis(1), "model");
        inference::assign(&v.conn, "a", &axis(0), "A", "model").unwrap();
        inference::assign(&v.conn, "b", &axis(1), "B", "model").unwrap();
        v.conn
            .execute("DELETE FROM atoms WHERE id='a'", [])
            .unwrap();
        let labels: Vec<String> = v
            .conn
            .prepare("SELECT label FROM topics ORDER BY label")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(labels, vec!["Title b"]);
    }

    #[test]
    fn v1_trigger_migrates_without_erasing_other_topics() {
        let (dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "model");
        insert_atom(&v, "b", "2026-01-02T00:00:00Z", &axis(1), "model");
        inference::assign(&v.conn, "a", &axis(0), "A", "model").unwrap();
        inference::assign(&v.conn, "b", &axis(1), "B", "model").unwrap();
        v.conn
            .execute_batch(
                "DROP TRIGGER topic_invalidate;
CREATE TRIGGER topic_invalidate AFTER DELETE ON atoms BEGIN DELETE FROM topics; END;
PRAGMA user_version=1;",
            )
            .unwrap();
        drop(v);
        let upgraded = Vault::open(&dir.path().join("vault.sqlite")).unwrap();
        let version: i64 = upgraded
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
        upgraded
            .conn
            .execute("DELETE FROM atoms WHERE id='a'", [])
            .unwrap();
        let labels: Vec<String> = upgraded
            .conn
            .prepare("SELECT label FROM topics ORDER BY label")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(labels, vec!["Title b"]);
    }

    #[test]
    fn v2_vault_adds_topic_skip_tracking_without_losing_atoms() {
        let (dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "model");
        v.conn.execute_batch("DROP TRIGGER topic_deleted_reconsider_skips; DROP TABLE topic_skips; PRAGMA user_version=2;").unwrap();
        drop(v);
        let upgraded = Vault::open(&dir.path().join("vault.sqlite")).unwrap();
        let version: i64 = upgraded
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let atoms: i64 = upgraded
            .conn
            .query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
        assert_eq!(atoms, 1);
        upgraded
            .conn
            .execute("INSERT INTO topic_skips VALUES('a','model')", [])
            .unwrap();
    }

    #[test]
    fn topic_label_follows_current_representative() {
        let (_dir, v) = vault();
        let first = axis(0);
        let mut later = vec![0.0; 256];
        later[0] = 0.8;
        later[1] = 0.6;
        for atom in ["a", "b", "c"] {
            let vector = if atom == "a" { &first } else { &later };
            insert_atom(&v, atom, "2026-01-01T00:00:00Z", vector, "model");
            inference::assign(&v.conn, atom, vector, &format!("Title {atom}"), "model").unwrap();
        }
        let label: String = v
            .conn
            .query_row("SELECT label FROM topics", [], |r| r.get(0))
            .unwrap();
        assert_eq!(label, "Title b");
    }

    #[test]
    fn topic_capacity_skip_does_not_leave_permanent_pending_work() {
        let (_dir, v) = vault();
        for index in 0..=algorithm::MAX_TOPICS {
            let atom = format!("atom-{index:03}");
            let vector = axis(index);
            insert_atom(&v, &atom, "2026-01-01T00:00:00Z", &vector, "model");
            inference::assign(&v.conn, &atom, &vector, &atom, "model").unwrap();
        }
        assert_eq!(v.pending_atoms_for_model(Some("model")).unwrap(), 0);
        let skipped: i64 = v
            .conn
            .query_row("SELECT count(*) FROM topic_skips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(skipped, 1);
        v.conn
            .execute("DELETE FROM atoms WHERE id='atom-000'", [])
            .unwrap();
        assert_eq!(v.pending_atoms_for_model(Some("model")).unwrap(), 1);
    }
}
