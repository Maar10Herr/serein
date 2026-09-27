//! Bounded topic inference. Scores describe activity, never belief probabilities.
use crate::*;
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
pub const ADMISSION: f32 = 0.62;
pub const MARGIN: f32 = 0.08;
pub const RUNNER_UP: f32 = 0.55;
fn decode(b: Vec<u8>) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|x| f32::from_le_bytes(x.try_into().unwrap()))
        .collect()
}
fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
pub fn assign(
    conn: &Connection,
    atom: &str,
    vector: &[f32],
    label: &str,
    model_hash: &str,
) -> Result<()> {
    let mut candidates = conn
        .prepare("SELECT id,centroid FROM topics WHERE model=? ORDER BY id LIMIT 128")?
        .query_map([model_hash], |r| {
            Ok((r.get::<_, String>(0)?, decode(r.get(1)?)))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    candidates.sort_by(|a, b| {
        model::cosine(&b.1, vector)
            .total_cmp(&model::cosine(&a.1, vector))
            .then(a.0.cmp(&b.0))
    });
    let scores: Vec<_> = candidates
        .iter()
        .map(|(id, v)| (id.clone(), model::cosine(v, vector)))
        .take(2)
        .collect();
    let assignments = if scores.first().is_some_and(|(_, s)| *s >= ADMISSION) {
        if scores.len() == 2 && scores[0].1 - scores[1].1 < MARGIN && scores[1].1 >= RUNNER_UP {
            let a = (10.0 * scores[0].1).exp();
            let b = (10.0 * scores[1].1).exp();
            vec![
                (scores[0].0.clone(), a / (a + b)),
                (scores[1].0.clone(), b / (a + b)),
            ]
        } else {
            vec![(scores[0].0.clone(), 1.0)]
        }
    } else if candidates.len() < 128 {
        let topic = id();
        conn.execute(
            "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
            params![
                topic,
                label.chars().take(72).collect::<String>(),
                encode(vector),
                model_hash
            ],
        )?;
        vec![(topic, 1.0)]
    } else {
        vec![]
    };
    for (topic, mass) in assignments {
        conn.execute(
            "INSERT OR REPLACE INTO atom_topics VALUES(?,?,?)",
            params![atom, topic, mass],
        )?;
        recompute(conn, &topic, model_hash)?;
    }
    Ok(())
}
fn recompute(conn: &Connection, topic: &str, model_hash: &str) -> Result<()> {
    let rows=conn.prepare("SELECT a.site,d.session,d.mass,m.mass,v.vector FROM atom_topics m JOIN atoms a ON a.id=m.atom JOIN vectors v ON v.atom=a.id JOIN atom_days d ON d.atom=a.id WHERE m.topic=? AND v.model=?")?.query_map(params![topic,model_hash],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,f32>(2)?*r.get::<_,f32>(3)?,decode(r.get(4)?))))?.collect::<std::result::Result<Vec<_>,_>>()?;
    let mut buckets: BTreeMap<(String, String), (Vec<f32>, f32)> = BTreeMap::new();
    for (site, session, mass, vector) in rows {
        let b = buckets
            .entry((site, session))
            .or_insert((vec![0.0; 256], 0.0));
        for (v, x) in b.0.iter_mut().zip(vector) {
            *v += mass * x
        }
        b.1 += mass;
    }
    let mut centroid = vec![0.0; 256];
    for (v, mass) in buckets.values() {
        if *mass > 0.0 {
            for (c, x) in centroid.iter_mut().zip(v) {
                *c += x / mass * mass.min(1.0)
            }
        }
    }
    if let Some(v) = model::normalize(centroid) {
        conn.execute(
            "UPDATE topics SET centroid=? WHERE id=?",
            params![encode(&v), topic],
        )?;
    }
    Ok(())
}
pub fn activity(conn: &Connection) -> Result<serde_json::Value> {
    let mut topics = vec![];
    let rows = conn
        .prepare("SELECT id,label FROM topics ORDER BY id LIMIT 128")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut total = [0f64; 3];
    for (id, label) in rows {
        let buckets=conn.prepare("SELECT a.site,d.session,d.day,min(1.0,sum(d.mass*m.mass)) FROM atom_topics m JOIN atoms a ON a.id=m.atom JOIN atom_days d ON d.atom=a.id WHERE m.topic=? GROUP BY a.site,d.session,d.day")?.query_map([&id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,f64>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut masses = [0f64; 3];
        let mut sessions = std::collections::HashSet::new();
        let mut sites = std::collections::HashSet::new();
        for (site, session, day, mass) in buckets {
            sites.insert(site);
            sessions.insert(session);
            if let Ok(day) = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d") {
                let age = chrono::Utc::now()
                    .date_naive()
                    .signed_duration_since(day)
                    .num_days()
                    .max(0) as f64;
                for (i, h) in [1.0, 7.0, 30.0].iter().enumerate() {
                    masses[i] += mass * 2f64.powf(-age / h)
                }
            }
        }
        for i in 0..3 {
            total[i] += masses[i]
        }
        topics.push(serde_json::json!({"id":id,"label":label,"state":"suggested","candidate":sessions.len()>=2,"sessions":sessions.len(),"sites":sites.len(),"mass":masses}));
    }
    let n = topics.len().max(1) as f64;
    for t in &mut topics {
        let mut shares = [0f64; 3];
        for i in 0..3 {
            shares[i] = (2.0 / n + t["mass"][i].as_f64().unwrap_or(0.0)) / (2.0 + total[i])
        }
        t["activity_share"] = serde_json::json!(shares);
        t["burst"] = serde_json::json!(((shares[0] + 1e-6) / (shares[2] + 1e-6)).ln());
    }
    Ok(serde_json::json!(topics))
}
