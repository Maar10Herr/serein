//! Bounded topic inference. Scores describe activity, never belief probabilities.
use crate::algorithm;
use crate::*;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
/// Convert a decayed daily mass to its equivalent steady daily arrival rate.
fn daily_rate(mass: f64, half_life_days: f64) -> f64 {
    mass * (1.0 - 2f64.powf(-1.0 / half_life_days))
}
fn burst(mass_one_day: f64, mass_thirty_days: f64) -> f64 {
    if mass_one_day <= 0.0 && mass_thirty_days <= 0.0 {
        return 0.0;
    }
    // A small daily-rate prior prevents a single observation from dominating.
    let recent = daily_rate(mass_one_day, 1.0);
    let baseline = daily_rate(mass_thirty_days, 30.0);
    ((recent + algorithm::BURST_DAILY_RATE_PRIOR) / (baseline + algorithm::BURST_DAILY_RATE_PRIOR))
        .ln()
}
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
    let previous = conn
        .prepare("SELECT topic FROM atom_topics WHERE atom=? ORDER BY topic")?
        .query_map([atom], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    conn.execute("DELETE FROM atom_topics WHERE atom=?", [atom])?;
    for topic in previous {
        recompute(conn, &topic, model_hash)?;
    }
    let mut candidates = conn
        .prepare("SELECT id,centroid FROM topics WHERE model=? ORDER BY id LIMIT ?")?
        .query_map(params![model_hash, algorithm::MAX_TOPICS as i64], |r| {
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
    let assignments = if scores
        .first()
        .is_some_and(|(_, s)| *s >= algorithm::TOPIC_ADMISSION)
    {
        if scores.len() == 2
            && scores[0].1 - scores[1].1 < algorithm::TOPIC_MARGIN
            && scores[1].1 >= algorithm::RUNNER_UP_ADMISSION
        {
            let a = (10.0 * scores[0].1).exp();
            let b = (10.0 * scores[1].1).exp();
            vec![
                (scores[0].0.clone(), a / (a + b)),
                (scores[1].0.clone(), b / (a + b)),
            ]
        } else {
            vec![(scores[0].0.clone(), 1.0)]
        }
    } else if candidates.len() < algorithm::MAX_TOPICS {
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&Sha256::digest(format!("{model_hash}:{atom}").as_bytes())[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let topic = uuid::Uuid::from_bytes(bytes).to_string();
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
    if assignments.is_empty() {
        conn.execute(
            "INSERT INTO topic_skips(atom,model) VALUES(?,?) ON CONFLICT(atom) DO UPDATE SET model=excluded.model",
            params![atom, model_hash],
        )?;
    } else {
        conn.execute("DELETE FROM topic_skips WHERE atom=?", [atom])?;
    }
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
    if rows.is_empty() {
        conn.execute("DELETE FROM topics WHERE id=?", [topic])?;
        return Ok(());
    }
    let mut buckets: BTreeMap<(String, String), (Vec<f32>, f32)> = BTreeMap::new();
    for (site, session, mass, vector) in rows {
        let b = buckets
            .entry((site, session))
            .or_insert((vec![0.0; algorithm::DIMENSIONS], 0.0));
        for (v, x) in b.0.iter_mut().zip(vector) {
            *v += mass * x
        }
        b.1 += mass;
    }
    let mut centroid = vec![0.0; algorithm::DIMENSIONS];
    for (v, mass) in buckets.values() {
        if *mass > 0.0 {
            for (c, x) in centroid.iter_mut().zip(v) {
                *c += x / mass * mass.min(1.0)
            }
        }
    }
    if let Some(v) = model::normalize(centroid) {
        let members = conn.prepare("SELECT a.id,a.title,a.query,v.vector FROM atom_topics m JOIN atoms a ON a.id=m.atom JOIN vectors v ON v.atom=a.id WHERE m.topic=? AND v.model=? ORDER BY a.first_seen,a.id")?
            .query_map(params![topic,model_hash],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,decode(r.get(3)?))))?
            .collect::<std::result::Result<Vec<_>,_>>()?;
        let representative = members.into_iter().max_by(|a, b| {
            model::cosine(&a.3, &v)
                .total_cmp(&model::cosine(&b.3, &v))
                .then_with(|| b.0.cmp(&a.0))
        });
        let label = representative
            .map(|(_, title, query, _)| query.unwrap_or(title))
            .unwrap_or_default()
            .chars()
            .take(72)
            .collect::<String>();
        conn.execute(
            "UPDATE topics SET centroid=?,label=? WHERE id=?",
            params![encode(&v), label, topic],
        )?;
    }
    Ok(())
}
pub fn activity(conn: &Connection, active_model_hash: Option<&str>) -> Result<serde_json::Value> {
    let Some(active_model_hash) = active_model_hash else {
        return Ok(serde_json::json!([]));
    };
    let mut topics = vec![];
    let rows = conn
        .prepare("SELECT id,label FROM topics WHERE model=? ORDER BY id LIMIT ?")?
        .query_map(
            params![active_model_hash, algorithm::MAX_TOPICS as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
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
                for (i, h) in algorithm::ACTIVITY_HALF_LIVES_DAYS.iter().enumerate() {
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
            shares[i] = (algorithm::ACTIVITY_PRIOR_STRENGTH / n
                + t["mass"][i].as_f64().unwrap_or(0.0))
                / (algorithm::ACTIVITY_PRIOR_STRENGTH + total[i])
        }
        t["activity_share"] = serde_json::json!(shares);
        t["burst"] = serde_json::json!(burst(
            t["mass"][0].as_f64().unwrap_or(0.0),
            t["mass"][2].as_f64().unwrap_or(0.0),
        ));
    }
    Ok(serde_json::json!(topics))
}
#[cfg(test)]
mod burst_tests {
    use super::*;

    fn history(days: usize, daily_mass: impl Fn(usize) -> f64) -> (f64, f64) {
        let mass = |half_life: f64| {
            (0..days)
                .map(|age| daily_mass(age) * 2f64.powf(-(age as f64) / half_life))
                .sum()
        };
        (mass(1.0), mass(30.0))
    }

    #[test]
    fn inactive_topic_has_zero_burst() {
        assert_eq!(burst(0.0, 0.0), 0.0);
    }

    #[test]
    fn stationary_activity_has_near_zero_expected_burst() {
        for level in [0.25, 0.5, 1.0, 2.0, 5.0] {
            let (short, long) = history(365, |_| level);
            assert!(burst(short, long).abs() < 0.001, "level {level}");
        }
    }

    #[test]
    fn recent_acceleration_has_positive_burst() {
        let (short, long) = history(365, |age| if age < 3 { 5.0 } else { 1.0 });
        assert!(burst(short, long) > 0.5);
    }

    #[test]
    fn recent_deceleration_has_negative_burst() {
        let (short, long) = history(365, |age| if age < 3 { 0.0 } else { 1.0 });
        assert!(burst(short, long) < -0.5);
    }
}
