//! Bounded topic inference. Scores describe activity, never belief probabilities.
use crate::algorithm;
use crate::*;
use rusqlite::{params, types::ValueRef, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Instant;
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
fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
const CACHE_BUCKET_LIMIT: usize = 4096;
const CACHE_HASH_BYTES: usize = 32;
const CENTROID_GUARD: f32 = 1.0e-4;
const WAL_FRAME_OVERHEAD_ESTIMATE: u64 = 32;
const ZERO_SUM: &[u8] = &[0; algorithm::DIMENSIONS * 8];

#[cfg(test)]
std::thread_local! {
    static REFERENCE_ROWS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static DELTA_ROWS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static LABEL_ROWS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[inline]
fn note_reference_row_read() {
    #[cfg(test)]
    REFERENCE_ROWS_READ.with(|count| count.set(count.get() + 1));
}

#[inline]
fn note_delta_row_read() {
    #[cfg(test)]
    DELTA_ROWS_READ.with(|count| count.set(count.get() + 1));
}

#[inline]
fn note_label_row_read() {
    #[cfg(test)]
    LABEL_ROWS_READ.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(crate) fn reset_work_counters() {
    REFERENCE_ROWS_READ.with(|count| count.set(0));
    DELTA_ROWS_READ.with(|count| count.set(0));
    LABEL_ROWS_READ.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn work_counters() -> (u64, u64, u64) {
    (
        REFERENCE_ROWS_READ.with(std::cell::Cell::get),
        DELTA_ROWS_READ.with(std::cell::Cell::get),
        LABEL_ROWS_READ.with(std::cell::Cell::get),
    )
}

type BucketKey = (String, String);

#[derive(Clone)]
struct Bucket {
    mass: f64,
    resultant: Vec<f64>,
}

struct ReferenceTopic {
    sum: Vec<f64>,
    buckets: Option<BTreeMap<BucketKey, Bucket>>,
    centroid: Option<Vec<f32>>,
}

enum ReferenceResult {
    Empty,
    Invalid,
    Ready(ReferenceTopic),
}

#[derive(Clone)]
struct Candidate {
    id: String,
    centroid: Vec<f32>,
    score: f32,
}

fn vector_from_blob(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.len() != algorithm::DIMENSIONS * 4 {
        return None;
    }
    let vector: Vec<_> = blob
        .chunks_exact(4)
        .map(|component| f32::from_le_bytes(component.try_into().unwrap()))
        .collect();
    vector
        .iter()
        .all(|component| component.is_finite())
        .then_some(vector)
}

fn decode_f64(blob: &[u8]) -> Option<Vec<f64>> {
    if blob.len() != algorithm::DIMENSIONS * 8 {
        return None;
    }
    let vector: Vec<_> = blob
        .chunks_exact(8)
        .map(|component| f64::from_le_bytes(component.try_into().unwrap()))
        .collect();
    vector
        .iter()
        .all(|component| component.is_finite())
        .then_some(vector)
}

fn cache_blob(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Blob(value) => Some(value.to_vec()),
        _ => None,
    })
}

fn cache_text(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<String>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Text(value) => Some(String::from_utf8_lossy(value).into_owned()),
        _ => None,
    })
}

fn cache_integer(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<i64>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Integer(value) => Some(value),
        _ => None,
    })
}

fn cache_number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<f64>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Integer(value) => Some(value as f64),
        ValueRef::Real(value) => Some(value),
        _ => None,
    })
}

fn encode_f64(vector: &[f64]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|component| component.to_le_bytes())
        .collect()
}

fn cache_sum_hash(model_hash: &str, sum_blob: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(model_hash.as_bytes());
    digest.update([0]);
    digest.update(sum_blob);
    digest.finalize().to_vec()
}

fn cache_bucket_hash(
    topic: &str,
    model_hash: &str,
    key: &BucketKey,
    mass: f64,
    resultant_blob: &[u8],
) -> Vec<u8> {
    let mut digest = Sha256::new();
    for part in [
        topic.as_bytes(),
        model_hash.as_bytes(),
        key.0.as_bytes(),
        key.1.as_bytes(),
    ] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    digest.update(mass.to_le_bytes());
    digest.update(resultant_blob);
    digest.finalize().to_vec()
}

fn centroid_from_sum(sum: &[f64]) -> Option<Vec<f32>> {
    if sum.len() != algorithm::DIMENSIONS || sum.iter().any(|component| !component.is_finite()) {
        return None;
    }
    let sum = sum
        .iter()
        .map(|component| *component as f32)
        .collect::<Vec<_>>();
    if sum.iter().any(|component| !component.is_finite()) {
        return None;
    }
    model::normalize(sum)
}

fn bucket_group(bucket: &Bucket) -> Option<Vec<f64>> {
    if !bucket.mass.is_finite()
        || bucket.mass < 0.0
        || bucket.resultant.len() != algorithm::DIMENSIONS
        || bucket
            .resultant
            .iter()
            .any(|component| !component.is_finite())
    {
        return None;
    }
    if bucket.mass == 0.0 {
        return bucket
            .resultant
            .iter()
            .all(|component| *component == 0.0)
            .then(|| vec![0.0; algorithm::DIMENSIONS]);
    }
    let scale = bucket.mass.min(1.0) / bucket.mass;
    let result = bucket
        .resultant
        .iter()
        .map(|component| component * scale)
        .collect::<Vec<_>>();
    result
        .iter()
        .all(|component| component.is_finite())
        .then_some(result)
}

fn read_cache_sum_fields(
    conn: &Connection,
    topic: &str,
) -> Result<
    Option<(
        Option<String>,
        Option<i64>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    )>,
> {
    Ok(conn
        .query_row(
            "SELECT model,valid,sum,sum_hash FROM topic_cache WHERE topic=?",
            [topic],
            |row| {
                Ok((
                    cache_text(row, 0)?,
                    cache_integer(row, 1)?,
                    cache_blob(row, 2)?,
                    cache_blob(row, 3)?,
                ))
            },
        )
        .optional()?)
}

fn decode_cache_sum_fields(
    fields: Option<(
        Option<String>,
        Option<i64>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    )>,
    model_hash: &str,
    require_valid: bool,
) -> Option<Vec<f64>> {
    let (Some(cached_model), valid, Some(sum_blob), Some(sum_hash)) = fields? else {
        return None;
    };
    if cached_model != model_hash
        || (require_valid && valid != Some(1))
        || sum_hash.len() != CACHE_HASH_BYTES
        || cache_sum_hash(model_hash, &sum_blob) != sum_hash
    {
        return None;
    }
    decode_f64(&sum_blob)
}

fn read_valid_sum(conn: &Connection, topic: &str, model_hash: &str) -> Result<Option<Vec<f64>>> {
    let fields = read_cache_sum_fields(conn, topic)?;
    Ok(decode_cache_sum_fields(fields, model_hash, true))
}

fn read_sum_even_if_invalid(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
) -> Result<Option<Vec<f64>>> {
    let fields = read_cache_sum_fields(conn, topic)?;
    Ok(decode_cache_sum_fields(fields, model_hash, false))
}

fn mark_cache_invalid(conn: &Connection, topic: &str) -> Result<()> {
    conn.execute(
        "UPDATE topic_cache SET valid=0,dirty_label=1 WHERE topic=?",
        [topic],
    )?;
    Ok(())
}

fn cache_bucket_count(conn: &Connection) -> Result<usize> {
    Ok(
        conn.query_row("SELECT count(*) FROM topic_cache_buckets", [], |row| {
            row.get::<_, i64>(0)
        })? as usize,
    )
}

fn topic_bucket_count(conn: &Connection, topic: &str) -> Result<usize> {
    Ok(conn.query_row(
        "SELECT count(*) FROM topic_cache_buckets WHERE topic=?",
        [topic],
        |row| row.get::<_, i64>(0),
    )? as usize)
}

fn page_write_bytes(conn: &Connection, pages: u64) -> Result<u64> {
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(pages.saturating_mul(page_size.max(512) as u64 + WAL_FRAME_OVERHEAD_ESTIMATE))
}

fn estimated_reference_cache_bytes(conn: &Connection, bucket_rows: usize) -> Result<u64> {
    // Each bucket may dirty a table page and a primary-key index page. Reserve
    // the same two pages for the accumulator row and its topic key.
    page_write_bytes(
        conn,
        (bucket_rows as u64).saturating_mul(2).saturating_add(2),
    )
}

fn cached_topic_delete_bytes(conn: &Connection, topic: &str) -> Result<u64> {
    let buckets = topic_bucket_count(conn, topic)?;
    let has_accumulator: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM topic_cache WHERE topic=?)",
        [topic],
        |row| row.get(0),
    )?;
    if !has_accumulator {
        return Ok(0);
    }
    page_write_bytes(conn, (buckets as u64).saturating_mul(2).saturating_add(2))
}

fn cache_eviction_plan(
    conn: &Connection,
    keep_topic: &str,
    projected_bucket_rows: usize,
) -> Result<Option<Vec<String>>> {
    if projected_bucket_rows <= CACHE_BUCKET_LIMIT {
        return Ok(Some(Vec::new()));
    }
    let candidates = conn
        .prepare(
            "SELECT c.topic,count(b.topic) AS bucket_count
             FROM topic_cache c
             JOIN topic_cache_buckets b ON b.topic=c.topic
             WHERE c.topic<>?
             GROUP BY c.topic ORDER BY c.topic",
        )?
        .query_map([keep_topic], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut projected = projected_bucket_rows;
    let mut victims = Vec::new();
    for (topic, bucket_rows) in candidates {
        if projected <= CACHE_BUCKET_LIMIT {
            break;
        }
        projected = projected.saturating_sub(bucket_rows);
        victims.push(topic);
    }
    Ok((projected <= CACHE_BUCKET_LIMIT).then_some(victims))
}

fn delete_cached_topics(conn: &Connection, topics: &[String], headroom: &mut u64) -> Result<bool> {
    let mut required = 0u64;
    for topic in topics {
        required = required.saturating_add(cached_topic_delete_bytes(conn, topic)?);
    }
    if required > *headroom {
        return Ok(false);
    }
    for topic in topics {
        conn.execute("DELETE FROM topic_cache WHERE topic=?", [topic])?;
    }
    *headroom = headroom.saturating_sub(required);
    Ok(true)
}

fn store_reference_cache(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    sum: &[f64],
    buckets: &BTreeMap<BucketKey, Bucket>,
    headroom: &mut u64,
) -> Result<bool> {
    if buckets.len() > CACHE_BUCKET_LIMIT {
        mark_cache_invalid(conn, topic)?;
        let _ = delete_cached_topics(conn, &[topic.to_owned()], headroom)?;
        return Ok(false);
    }
    let insert_bytes = estimated_reference_cache_bytes(conn, buckets.len())?;
    let global_count = cache_bucket_count(conn)?;
    let existing_count = topic_bucket_count(conn, topic)?;
    let projected = global_count
        .saturating_sub(existing_count)
        .saturating_add(buckets.len());
    let Some(victims) = cache_eviction_plan(conn, topic, projected)? else {
        mark_cache_invalid(conn, topic)?;
        return Ok(false);
    };
    let mut removals = Vec::with_capacity(victims.len() + 1);
    let has_current: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM topic_cache WHERE topic=?)",
        [topic],
        |row| row.get(0),
    )?;
    if has_current {
        removals.push(topic.to_owned());
    }
    removals.extend(victims);
    let mut delete_bytes = 0u64;
    for remove_topic in &removals {
        delete_bytes = delete_bytes.saturating_add(cached_topic_delete_bytes(conn, remove_topic)?);
    }
    let required = insert_bytes.saturating_add(delete_bytes);
    if required > *headroom {
        mark_cache_invalid(conn, topic)?;
        return Ok(false);
    }
    for remove_topic in &removals {
        conn.execute("DELETE FROM topic_cache WHERE topic=?", [remove_topic])?;
    }

    let sum_blob = encode_f64(sum);
    let sum_hash = cache_sum_hash(model_hash, &sum_blob);
    conn.execute(
        "INSERT INTO topic_cache(topic,model,valid,sum,sum_hash,dirty_label)
         VALUES(?,?,0,?,?,1)",
        params![topic, model_hash, sum_blob, sum_hash],
    )?;
    for (key, bucket) in buckets {
        let resultant = encode_f64(&bucket.resultant);
        let resultant_hash = cache_bucket_hash(topic, model_hash, key, bucket.mass, &resultant);
        conn.execute(
            "INSERT INTO topic_cache_buckets(topic,site,session,mass,resultant,resultant_hash)
             VALUES(?,?,?,?,?,?)",
            params![topic, key.0, key.1, bucket.mass, resultant, resultant_hash],
        )?;
    }
    conn.execute(
        "UPDATE topic_cache SET valid=1,dirty_label=1 WHERE topic=? AND model=?",
        params![topic, model_hash],
    )?;
    *headroom = headroom.saturating_sub(required);
    Ok(true)
}

fn finalize_reference_bucket(
    key: &BucketKey,
    mass: f64,
    resultant: &[f64],
    sum: &mut [f64],
    buckets: &mut BTreeMap<BucketKey, Bucket>,
    cacheable: &mut bool,
) -> bool {
    let bucket = Bucket {
        mass,
        resultant: resultant.to_vec(),
    };
    let Some(group) = bucket_group(&bucket) else {
        return false;
    };
    for (total, component) in sum.iter_mut().zip(group) {
        *total += component;
        if !total.is_finite() {
            return false;
        }
    }
    if mass > 0.0 && *cacheable {
        if buckets.len() >= CACHE_BUCKET_LIMIT {
            *cacheable = false;
            buckets.clear();
        } else {
            buckets.insert(key.clone(), bucket);
        }
    }
    true
}

fn build_reference_topic(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
) -> Result<ReferenceResult> {
    let mut statement = conn.prepare(
        "SELECT a.site,d.session,d.mass,m.mass,v.vector
         FROM atom_topics m
         JOIN atoms a ON a.id=m.atom
         JOIN vectors v ON v.atom=a.id AND v.model=?2
         JOIN atom_days d ON d.atom=a.id
         WHERE m.topic=?1
         ORDER BY a.site,d.session,a.id,d.day",
    )?;
    let mut rows = statement.query(params![topic, model_hash])?;
    let mut sum = vec![0.0_f64; algorithm::DIMENSIONS];
    let mut buckets = BTreeMap::new();
    let mut cacheable = true;
    let mut found_rows = 0usize;
    let mut current_key: Option<BucketKey> = None;
    let mut current_mass = 0.0_f64;
    let mut current_resultant = vec![0.0_f64; algorithm::DIMENSIONS];

    while let Some(row) = rows.next()? {
        note_reference_row_read();
        found_rows += 1;
        let key = (row.get::<_, String>(0)?, row.get::<_, String>(1)?);
        if current_key
            .as_ref()
            .is_some_and(|previous| previous != &key)
        {
            let previous = current_key.as_ref().unwrap();
            if !finalize_reference_bucket(
                previous,
                current_mass,
                &current_resultant,
                &mut sum,
                &mut buckets,
                &mut cacheable,
            ) {
                return Ok(ReferenceResult::Invalid);
            }
            current_mass = 0.0;
            current_resultant.fill(0.0);
            current_key = Some(key);
        } else if current_key.is_none() {
            current_key = Some(key);
        }

        let day_mass: f64 = row.get(2)?;
        let membership_mass: f64 = row.get(3)?;
        if !day_mass.is_finite()
            || !membership_mass.is_finite()
            || day_mass < 0.0
            || membership_mass < 0.0
        {
            return Ok(ReferenceResult::Invalid);
        }
        let Some(vector) = vector_from_blob(&row.get::<_, Vec<u8>>(4)?) else {
            return Ok(ReferenceResult::Invalid);
        };
        let weight = day_mass * membership_mass;
        if !weight.is_finite() || weight < 0.0 {
            return Ok(ReferenceResult::Invalid);
        }
        current_mass += weight;
        if !current_mass.is_finite() {
            return Ok(ReferenceResult::Invalid);
        }
        for (component, value) in current_resultant.iter_mut().zip(vector) {
            *component += weight * value as f64;
            if !component.is_finite() {
                return Ok(ReferenceResult::Invalid);
            }
        }
    }
    if found_rows == 0 {
        return Ok(ReferenceResult::Empty);
    }
    if let Some(key) = current_key.as_ref() {
        if !finalize_reference_bucket(
            key,
            current_mass,
            &current_resultant,
            &mut sum,
            &mut buckets,
            &mut cacheable,
        ) {
            return Ok(ReferenceResult::Invalid);
        }
    }
    let centroid = centroid_from_sum(&sum);
    Ok(ReferenceResult::Ready(ReferenceTopic {
        sum,
        buckets: cacheable.then_some(buckets),
        centroid,
    }))
}

fn rebuild_from_reference(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    headroom: &mut u64,
) -> Result<Option<Vec<f32>>> {
    match build_reference_topic(conn, topic, model_hash)? {
        ReferenceResult::Empty => {
            // Remove the derived rows through the same headroom gate before
            // their parent topic is deleted. Otherwise the FK cascade could
            // grow the WAL past the space reserved for cache maintenance.
            if !delete_cached_topics(conn, &[topic.to_owned()], headroom)? {
                mark_cache_invalid(conn, topic)?;
                return Ok(None);
            }
            conn.execute("DELETE FROM topics WHERE id=?", [topic])?;
            Ok(None)
        }
        ReferenceResult::Invalid => {
            mark_cache_invalid(conn, topic)?;
            let _ = delete_cached_topics(conn, &[topic.to_owned()], headroom)?;
            Ok(None)
        }
        ReferenceResult::Ready(reference) => {
            if let Some(centroid) = reference.centroid.as_ref() {
                let centroid_bytes = page_write_bytes(conn, 2)?;
                if centroid_bytes <= *headroom {
                    conn.execute(
                        "UPDATE topics SET centroid=? WHERE id=? AND model=?",
                        params![encode(centroid), topic, model_hash],
                    )?;
                    *headroom = headroom.saturating_sub(centroid_bytes);
                }
            }
            if let Some(buckets) = reference.buckets.as_ref() {
                if !store_reference_cache(
                    conn,
                    topic,
                    model_hash,
                    &reference.sum,
                    buckets,
                    headroom,
                )? {
                    mark_cache_invalid(conn, topic)?;
                }
            } else {
                if !delete_cached_topics(conn, &[topic.to_owned()], headroom)? {
                    mark_cache_invalid(conn, topic)?;
                }
            }
            Ok(reference.centroid)
        }
    }
}

fn ensure_topic_centroid(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    headroom: &mut u64,
) -> Result<Option<Vec<f32>>> {
    if let Some(sum) = read_valid_sum(conn, topic, model_hash)? {
        if let Some(centroid) = centroid_from_sum(&sum) {
            return Ok(Some(centroid));
        }
    }
    mark_cache_invalid(conn, topic)?;
    rebuild_from_reference(conn, topic, model_hash, headroom)
}

fn member_bucket_contributions(
    conn: &Connection,
    atom: &str,
    model_hash: &str,
    membership_mass: f64,
) -> Result<Option<BTreeMap<BucketKey, Bucket>>> {
    if !membership_mass.is_finite() || membership_mass < 0.0 {
        return Ok(None);
    }
    let mut statement = conn.prepare(
        "SELECT a.site,d.session,d.mass,v.vector
         FROM atoms a
         JOIN atom_days d ON d.atom=a.id
         JOIN vectors v ON v.atom=a.id
         WHERE a.id=? AND v.model=?
         ORDER BY a.site,d.session,d.day",
    )?;
    let mut rows = statement.query(params![atom, model_hash])?;
    let mut buckets = BTreeMap::<BucketKey, Bucket>::new();
    while let Some(row) = rows.next()? {
        note_delta_row_read();
        let key = (row.get::<_, String>(0)?, row.get::<_, String>(1)?);
        if !buckets.contains_key(&key) && buckets.len() >= CACHE_BUCKET_LIMIT {
            // A single atom can have many activity windows. Stop before the
            // per-atom map itself exceeds the same hard bound as the cache.
            return Ok(None);
        }
        let day_mass: f64 = row.get(2)?;
        if !day_mass.is_finite() || day_mass < 0.0 {
            return Ok(None);
        }
        let Some(vector) = vector_from_blob(&row.get::<_, Vec<u8>>(3)?) else {
            return Ok(None);
        };
        let weight = day_mass * membership_mass;
        if !weight.is_finite() || weight < 0.0 {
            return Ok(None);
        }
        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
            mass: 0.0,
            resultant: vec![0.0; algorithm::DIMENSIONS],
        });
        bucket.mass += weight;
        if !bucket.mass.is_finite() {
            return Ok(None);
        }
        for (component, value) in bucket.resultant.iter_mut().zip(vector) {
            *component += weight * value as f64;
            if !component.is_finite() {
                return Ok(None);
            }
        }
    }
    buckets.retain(|_, bucket| bucket.mass > 0.0);
    Ok(Some(buckets))
}

fn load_cache_bucket(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    key: &BucketKey,
) -> Result<Option<Bucket>> {
    let row: Option<(Option<f64>, Option<Vec<u8>>, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT mass,resultant,resultant_hash FROM topic_cache_buckets
             WHERE topic=? AND site=? AND session=?",
            params![topic, key.0, key.1],
            |row| {
                Ok((
                    cache_number(row, 0)?,
                    cache_blob(row, 1)?,
                    cache_blob(row, 2)?,
                ))
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (Some(mass), Some(resultant_blob), Some(hash)) = row else {
        return Ok(Some(Bucket {
            mass: f64::NAN,
            resultant: Vec::new(),
        }));
    };
    if !mass.is_finite() || mass <= 0.0 || hash.len() != CACHE_HASH_BYTES {
        return Ok(Some(Bucket {
            mass: f64::NAN,
            resultant: Vec::new(),
        }));
    }
    if cache_bucket_hash(topic, model_hash, key, mass, &resultant_blob) != hash {
        return Ok(Some(Bucket {
            mass: f64::NAN,
            resultant: Vec::new(),
        }));
    }
    let Some(resultant) = decode_f64(&resultant_blob) else {
        return Ok(Some(Bucket {
            mass: f64::NAN,
            resultant: Vec::new(),
        }));
    };
    Ok(Some(Bucket { mass, resultant }))
}

fn apply_cached_delta(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    previous_sum: &[f64],
    deltas: &BTreeMap<BucketKey, Bucket>,
    direction: f64,
    headroom: &mut u64,
) -> Result<Option<Vec<f32>>> {
    let Some(cached_sum) = read_sum_even_if_invalid(conn, topic, model_hash)? else {
        return Ok(None);
    };
    let previous_blob = encode_f64(previous_sum);
    let cached_blob = encode_f64(&cached_sum);
    if cached_blob != previous_blob {
        return Ok(None);
    }

    let mut sum = previous_sum.to_vec();
    let mut changes = Vec::with_capacity(deltas.len());
    for (key, delta) in deltas {
        let old = load_cache_bucket(conn, topic, model_hash, key)?.unwrap_or(Bucket {
            mass: 0.0,
            resultant: vec![0.0; algorithm::DIMENSIONS],
        });
        let Some(old_group) = bucket_group(&old) else {
            return Ok(None);
        };
        let mut new = Bucket {
            mass: old.mass + direction * delta.mass,
            resultant: old
                .resultant
                .iter()
                .zip(&delta.resultant)
                .map(|(old, change)| old + direction * change)
                .collect(),
        };
        if !new.mass.is_finite()
            || new.mass < 0.0
            || new.resultant.iter().any(|component| !component.is_finite())
        {
            return Ok(None);
        }
        if new.mass == 0.0 {
            if new.resultant.iter().any(|component| *component != 0.0) {
                return Ok(None);
            }
            new.resultant.fill(0.0);
        }
        let Some(new_group) = bucket_group(&new) else {
            return Ok(None);
        };
        for ((total, old_group), new_group) in sum.iter_mut().zip(old_group).zip(new_group) {
            *total += new_group - old_group;
            if !total.is_finite() {
                return Ok(None);
            }
        }
        changes.push((key.clone(), (new.mass > 0.0).then_some(new)));
    }
    let Some(centroid) = centroid_from_sum(&sum) else {
        return Ok(None);
    };

    let current_topic_buckets = topic_bucket_count(conn, topic)?;
    let mut next_topic_buckets = current_topic_buckets;
    for (key, new_bucket) in &changes {
        let old_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM topic_cache_buckets WHERE topic=? AND site=? AND session=?)",
            params![topic, key.0, key.1],
            |row| row.get(0),
        )?;
        match (old_exists, new_bucket.is_some()) {
            (false, true) => next_topic_buckets += 1,
            (true, false) => next_topic_buckets -= 1,
            _ => {}
        }
    }
    if next_topic_buckets > CACHE_BUCKET_LIMIT {
        return Ok(None);
    }
    let global_count = cache_bucket_count(conn)?;
    let projected = global_count
        .saturating_sub(current_topic_buckets)
        .saturating_add(next_topic_buckets);
    let Some(evictions) = cache_eviction_plan(conn, topic, projected)? else {
        return Ok(None);
    };
    // Updates and deletes may write both the cache table and its key index;
    // the accumulator and published centroid add two more rows. Evictions are
    // charged for their full cascade before any cache row is changed.
    let changed_bytes = page_write_bytes(
        conn,
        (changes.len() as u64).saturating_mul(2).saturating_add(4),
    )?;
    let mut eviction_bytes = 0u64;
    for victim in &evictions {
        eviction_bytes = eviction_bytes.saturating_add(cached_topic_delete_bytes(conn, victim)?);
    }
    let required = changed_bytes.saturating_add(eviction_bytes);
    if required > *headroom {
        return Ok(None);
    }
    if !delete_cached_topics(conn, &evictions, headroom)? {
        return Ok(None);
    }

    for (key, new_bucket) in changes {
        if let Some(bucket) = new_bucket {
            let resultant = encode_f64(&bucket.resultant);
            let resultant_hash =
                cache_bucket_hash(topic, model_hash, &key, bucket.mass, &resultant);
            conn.execute(
                "INSERT INTO topic_cache_buckets(topic,site,session,mass,resultant,resultant_hash)
                 VALUES(?,?,?,?,?,?)
                 ON CONFLICT(topic,site,session) DO UPDATE SET
                     mass=excluded.mass,resultant=excluded.resultant,resultant_hash=excluded.resultant_hash",
                params![topic, key.0, key.1, bucket.mass, resultant, resultant_hash],
            )?;
        } else {
            conn.execute(
                "DELETE FROM topic_cache_buckets WHERE topic=? AND site=? AND session=?",
                params![topic, key.0, key.1],
            )?;
        }
    }
    let sum_blob = encode_f64(&sum);
    let sum_hash = cache_sum_hash(model_hash, &sum_blob);
    conn.execute(
        "UPDATE topic_cache SET valid=1,sum=?,sum_hash=?,dirty_label=1 WHERE topic=? AND model=?",
        params![sum_blob, sum_hash, topic, model_hash],
    )?;
    conn.execute(
        "UPDATE topics SET centroid=? WHERE id=? AND model=?",
        params![encode(&centroid), topic, model_hash],
    )?;
    *headroom = headroom.saturating_sub(changed_bytes);
    Ok(Some(centroid))
}

fn create_empty_cache(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    headroom: &mut u64,
) -> Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM topic_cache WHERE topic=?)",
        [topic],
        |row| row.get(0),
    )?;
    if exists {
        // A retained invalid row may still own buckets that could not be
        // deleted under the current WAL budget. Do not replace it: SQLite's
        // REPLACE semantics would cascade through those rows before reference
        // fallback has enough space to clean them up.
        return Ok(false);
    }
    let estimated = page_write_bytes(conn, 2)?;
    if estimated > *headroom {
        return Ok(false);
    }
    let sum_blob = ZERO_SUM.to_vec();
    conn.execute(
        "INSERT OR IGNORE INTO topic_cache(topic,model,valid,sum,sum_hash,dirty_label)
         VALUES(?,?,1,?,?,1)",
        params![
            topic,
            model_hash,
            sum_blob,
            cache_sum_hash(model_hash, ZERO_SUM)
        ],
    )?;
    *headroom = headroom.saturating_sub(estimated);
    Ok(true)
}

fn deterministic_topic_id(model_hash: &str, atom: &str, collision: usize) -> String {
    let seed = if collision == 0 {
        format!("{model_hash}:{atom}")
    } else {
        format!("{model_hash}:{atom}:{collision}")
    };
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&Sha256::digest(seed.as_bytes())[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn next_topic_id(conn: &Connection, model_hash: &str, atom: &str) -> Result<String> {
    for collision in 1..=algorithm::MAX_TOPICS {
        let candidate = deterministic_topic_id(model_hash, atom, collision);
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM topics WHERE id=?)",
            [&candidate],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(candidate);
        }
    }
    // There are at most MAX_TOPICS live topic IDs, so the collision sequence
    // above must find an unused deterministic ID before exhausting its range.
    unreachable!("deterministic topic collision range exhausted")
}

pub fn assign(
    conn: &Connection,
    atom: &str,
    vector: &[f32],
    label: &str,
    model_hash: &str,
) -> Result<()> {
    assign_with_cache_headroom(conn, atom, vector, label, model_hash, u64::MAX)
}

pub(crate) fn assign_with_cache_headroom(
    conn: &Connection,
    atom: &str,
    vector: &[f32],
    label: &str,
    model_hash: &str,
    headroom_bytes: u64,
) -> Result<()> {
    if conn.is_autocommit() {
        let tx = conn.unchecked_transaction()?;
        assign_in_transaction(&tx, atom, vector, label, model_hash, headroom_bytes)?;
        tx.commit()?;
        Ok(())
    } else {
        assign_in_transaction(conn, atom, vector, label, model_hash, headroom_bytes)
    }
}

fn assign_in_transaction(
    conn: &Connection,
    atom: &str,
    vector: &[f32],
    label: &str,
    model_hash: &str,
    headroom_bytes: u64,
) -> Result<()> {
    if vector.len() != algorithm::DIMENSIONS
        || vector.iter().any(|component| !component.is_finite())
    {
        return Ok(());
    }
    let mut headroom = headroom_bytes;
    let previous = conn
        .prepare("SELECT topic,mass FROM atom_topics WHERE atom=? ORDER BY topic")?
        .query_map([atom], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut previous_caches = Vec::with_capacity(previous.len());
    for (topic, mass) in &previous {
        let _ = ensure_topic_centroid(conn, topic, model_hash, &mut headroom)?;
        let snapshot = read_valid_sum(conn, topic, model_hash)?;
        let contribution = member_bucket_contributions(conn, atom, model_hash, *mass)?;
        previous_caches.push((topic.clone(), snapshot, contribution));
    }
    conn.execute("DELETE FROM atom_topics WHERE atom=?", [atom])?;
    for (topic, snapshot, contribution) in previous_caches {
        let updated = match (snapshot, contribution) {
            (Some(sum), Some(contribution)) => apply_cached_delta(
                conn,
                &topic,
                model_hash,
                &sum,
                &contribution,
                -1.0,
                &mut headroom,
            )?,
            _ => None,
        };
        if updated.is_none() {
            let _ = rebuild_from_reference(conn, &topic, model_hash, &mut headroom)?;
        } else if !topic_has_reference_rows(conn, &topic, model_hash)? {
            conn.execute("DELETE FROM topics WHERE id=?", [&topic])?;
        }
    }

    let topic_ids = conn
        .prepare("SELECT id FROM topics WHERE model=? ORDER BY id LIMIT ?")?
        .query_map(params![model_hash, algorithm::MAX_TOPICS as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut candidates = Vec::with_capacity(topic_ids.len());
    for id in topic_ids {
        if let Some(centroid) = ensure_topic_centroid(conn, &id, model_hash, &mut headroom)? {
            let score = model::cosine(&centroid, vector);
            if score.is_finite() {
                candidates.push(Candidate {
                    id,
                    centroid,
                    score,
                });
            }
        }
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));

    let guard_ids = reference_guard_ids(&candidates);
    if !guard_ids.is_empty() {
        let mut refreshed = Vec::with_capacity(candidates.len());
        for mut candidate in candidates {
            if guard_ids.contains(&candidate.id) {
                let Some(centroid) =
                    rebuild_from_reference(conn, &candidate.id, model_hash, &mut headroom)?
                else {
                    continue;
                };
                candidate.centroid = centroid;
                candidate.score = model::cosine(&candidate.centroid, vector);
            }
            if candidate.score.is_finite() {
                refreshed.push(candidate);
            }
        }
        candidates = refreshed;
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
    }
    let topic_count: usize = conn.query_row(
        "SELECT count(*) FROM topics WHERE model=?",
        [model_hash],
        |row| row.get::<_, i64>(0),
    )? as usize;
    let scores: Vec<_> = candidates.iter().take(2).collect();
    let assignments: Vec<(String, f64)> = if scores
        .first()
        .is_some_and(|candidate| candidate.score >= algorithm::TOPIC_ADMISSION)
    {
        if scores.len() == 2
            && scores[0].score - scores[1].score < algorithm::TOPIC_MARGIN
            && scores[1].score >= algorithm::RUNNER_UP_ADMISSION
        {
            let first = (10.0 * f64::from(scores[0].score)).exp();
            let second = (10.0 * f64::from(scores[1].score)).exp();
            vec![
                (scores[0].id.clone(), first / (first + second)),
                (scores[1].id.clone(), second / (first + second)),
            ]
        } else {
            vec![(scores[0].id.clone(), 1.0)]
        }
    } else if topic_count < algorithm::MAX_TOPICS {
        let initial_id = deterministic_topic_id(model_hash, atom, 0);
        let initial_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM topics WHERE id=?)",
            [&initial_id],
            |row| row.get(0),
        )?;
        let reuse_empty =
            initial_exists && !topic_has_reference_rows(conn, &initial_id, model_hash)?;
        let topic = if initial_exists && !reuse_empty {
            next_topic_id(conn, model_hash, atom)?
        } else {
            initial_id
        };
        let initial_centroid = model::normalize(vector.to_vec()).unwrap_or_else(|| vector.to_vec());
        if reuse_empty {
            let estimated = page_write_bytes(conn, 2)?;
            if estimated <= headroom {
                conn.execute(
                    "UPDATE topics SET label=?,centroid=?,model=? WHERE id=?",
                    params![
                        label.chars().take(72).collect::<String>(),
                        encode(&initial_centroid),
                        model_hash,
                        topic
                    ],
                )?;
                headroom = headroom.saturating_sub(estimated);
            }
        } else {
            conn.execute(
                "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
                params![
                    topic,
                    label.chars().take(72).collect::<String>(),
                    encode(&initial_centroid),
                    model_hash
                ],
            )?;
        }
        let _ = create_empty_cache(conn, &topic, model_hash, &mut headroom)?;
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
        let snapshot = read_valid_sum(conn, &topic, model_hash)?;
        let contribution = member_bucket_contributions(conn, atom, model_hash, mass)?;
        conn.execute(
            "INSERT OR REPLACE INTO atom_topics(atom,topic,mass) VALUES(?,?,?)",
            params![atom, topic, mass],
        )?;
        let updated = match (snapshot, contribution) {
            (Some(sum), Some(contribution)) => apply_cached_delta(
                conn,
                &topic,
                model_hash,
                &sum,
                &contribution,
                1.0,
                &mut headroom,
            )?,
            _ => None,
        };
        if updated.is_none() {
            let _ = rebuild_from_reference(conn, &topic, model_hash, &mut headroom)?;
        }
    }
    Ok(())
}

fn topic_has_reference_rows(conn: &Connection, topic: &str, model_hash: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM atom_topics m
             JOIN atoms a ON a.id=m.atom
             JOIN vectors v ON v.atom=a.id AND v.model=?2
             JOIN atom_days d ON d.atom=a.id
             WHERE m.topic=?1
         )",
        params![topic, model_hash],
        |row| row.get(0),
    )?)
}

fn reference_guard_ids(candidates: &[Candidate]) -> std::collections::HashSet<String> {
    let mut guarded = std::collections::HashSet::new();
    for candidate in candidates {
        if (candidate.score - algorithm::TOPIC_ADMISSION).abs() <= CENTROID_GUARD
            || (candidate.score - algorithm::RUNNER_UP_ADMISSION).abs() <= CENTROID_GUARD
        {
            guarded.insert(candidate.id.clone());
        }
    }
    if candidates.len() >= 2 {
        let first = &candidates[0];
        let second = &candidates[1];
        let difference = first.score - second.score;
        if difference.abs() <= CENTROID_GUARD
            || (difference - algorithm::TOPIC_MARGIN).abs() <= CENTROID_GUARD
        {
            guarded.insert(first.id.clone());
            guarded.insert(second.id.clone());
        }
    }
    guarded
}

fn topic_representative_label(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    centroid: Option<&[f32]>,
    deadline: Option<(Instant, u64)>,
) -> Result<Option<String>> {
    let mut statement = conn.prepare(
        "SELECT a.id,a.title,
                (SELECT f.text FROM feedback f
                 WHERE f.atom=a.id AND f.action='confirm_constraint'
                 ORDER BY f.seq DESC LIMIT 1),
                v.vector
         FROM atom_topics m
         JOIN atoms a ON a.id=m.atom
         JOIN vectors v ON v.atom=a.id AND v.model=?2
         WHERE m.topic=?1
           AND NOT EXISTS (
               SELECT 1 FROM feedback f
               WHERE f.atom=a.id
                 AND f.action IN ('do_not_use','wrong_topic','not_about_me','temporary_research')
           )
         ORDER BY a.first_seen,a.id
         LIMIT 10000",
    )?;
    let mut rows = statement.query(params![topic, model_hash])?;
    let mut best: Option<(f32, String, String)> = None;
    while let Some(row) = rows.next()? {
        note_label_row_read();
        if deadline.is_some_and(|(start, budget)| start.elapsed().as_millis() as u64 >= budget) {
            return Ok(None);
        }
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let confirmation: Option<String> = row.get(2)?;
        let text = if let Some(assertion) = confirmation {
            if !importance::groupable(&assertion, None) {
                continue;
            }
            assertion
        } else {
            title
        };
        let Some(vector) = vector_from_blob(&row.get::<_, Vec<u8>>(3)?) else {
            continue;
        };
        let score = centroid.map_or(0.0, |centroid| model::cosine(&vector, centroid));
        if !score.is_finite() {
            continue;
        }
        let candidate = (score, id, text);
        let replace = best.as_ref().is_none_or(|current| {
            candidate.0.total_cmp(&current.0).is_gt()
                || (candidate.0.total_cmp(&current.0).is_eq() && candidate.1 < current.1)
        });
        if replace {
            best = Some(candidate);
        }
    }
    Ok(Some(
        best.map(|(_, _, text)| text.chars().take(72).collect())
            .unwrap_or_else(|| "Research".to_owned()),
    ))
}

fn persist_topic_label(
    conn: &Connection,
    topic: &str,
    model_hash: &str,
    label: &str,
    headroom: &mut u64,
) -> Result<bool> {
    let cache_row: Option<Option<String>> = conn
        .query_row(
            "SELECT model FROM topic_cache WHERE topic=?",
            [topic],
            |row| cache_text(row, 0),
        )
        .optional()?;
    if cache_row.as_ref().is_some_and(Option::is_none) {
        return Ok(false);
    }
    let cached_model = cache_row.flatten();
    if cached_model
        .as_deref()
        .is_some_and(|cached| cached != model_hash)
    {
        return Ok(false);
    }
    let existing = cached_model.is_some();
    let estimated = page_write_bytes(conn, 4)?;
    if estimated > *headroom {
        return Ok(false);
    }
    if existing {
        conn.execute(
            "UPDATE topics SET label=? WHERE id=? AND model=?",
            params![label, topic, model_hash],
        )?;
        conn.execute(
            "UPDATE topic_cache SET dirty_label=0 WHERE topic=? AND model=?",
            params![topic, model_hash],
        )?;
    } else {
        conn.execute(
            "INSERT INTO topic_cache(topic,model,valid,sum,sum_hash,dirty_label)
             VALUES(?,?,0,?,?,1)",
            params![
                topic,
                model_hash,
                ZERO_SUM,
                cache_sum_hash(model_hash, ZERO_SUM)
            ],
        )?;
        conn.execute(
            "UPDATE topics SET label=? WHERE id=? AND model=?",
            params![label, topic, model_hash],
        )?;
        conn.execute(
            "UPDATE topic_cache SET dirty_label=0 WHERE topic=? AND model=?",
            params![topic, model_hash],
        )?;
    }
    *headroom = headroom.saturating_sub(estimated);
    Ok(true)
}

pub fn activity(conn: &Connection, active_model_hash: Option<&str>) -> Result<serde_json::Value> {
    activity_atomically(conn, active_model_hash, u64::MAX, None, None)
}

#[cfg(test)]
pub(crate) fn activity_with_cache_headroom(
    conn: &Connection,
    active_model_hash: Option<&str>,
    headroom_bytes: u64,
) -> Result<serde_json::Value> {
    activity_atomically(conn, active_model_hash, headroom_bytes, None, None)
}

pub(crate) fn activity_with_cache_headroom_for_atoms(
    conn: &Connection,
    active_model_hash: Option<&str>,
    headroom_bytes: u64,
    eligible_atom_ids: &[String],
) -> Result<serde_json::Value> {
    activity_atomically(
        conn,
        active_model_hash,
        headroom_bytes,
        None,
        Some(eligible_atom_ids),
    )
}

fn activity_atomically(
    conn: &Connection,
    active_model_hash: Option<&str>,
    headroom_bytes: u64,
    label_deadline: Option<(Instant, u64)>,
    eligible_atom_ids: Option<&[String]>,
) -> Result<serde_json::Value> {
    if !conn.is_autocommit() {
        return activity_with_limits(
            conn,
            active_model_hash,
            headroom_bytes,
            label_deadline,
            eligible_atom_ids,
        );
    }
    // Serialize the evidence read and derived label write against feedback and
    // policy mutations. A deferred read transaction could otherwise return a
    // label from an older snapshot when its persistence is skipped for low
    // headroom.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let result = activity_with_limits(
        &tx,
        active_model_hash,
        headroom_bytes,
        label_deadline,
        eligible_atom_ids,
    )?;
    tx.commit()?;
    Ok(result)
}

fn activity_with_limits(
    conn: &Connection,
    active_model_hash: Option<&str>,
    headroom_bytes: u64,
    label_deadline: Option<(Instant, u64)>,
    eligible_atom_ids: Option<&[String]>,
) -> Result<serde_json::Value> {
    let Some(active_model_hash) = active_model_hash else {
        return Ok(serde_json::json!([]));
    };
    if eligible_atom_ids.is_some_and(|atom_ids| atom_ids.is_empty()) {
        return Ok(serde_json::json!([]));
    }
    let mut headroom = headroom_bytes;
    let mut topics = vec![];
    let rows = conn
        .prepare(
            "SELECT t.id,t.label,c.model,c.dirty_label
             FROM topics t LEFT JOIN topic_cache c ON c.topic=t.id
             WHERE t.model=? ORDER BY t.id LIMIT ?",
        )?
        .query_map(
            params![active_model_hash, algorithm::MAX_TOPICS as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    cache_text(r, 2)?,
                    cache_integer(r, 3)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut total = [0f64; 3];
    for (id, stored_label, cached_model, dirty_label) in rows {
        let clean_label =
            cached_model.as_deref() == Some(active_model_hash) && dirty_label == Some(0);
        let label = if clean_label {
            stored_label
        } else if label_deadline
            .is_some_and(|(start, budget)| start.elapsed().as_millis() as u64 >= budget)
        {
            "Research".to_owned()
        } else {
            let centroid = ensure_topic_centroid(conn, &id, active_model_hash, &mut headroom)?;
            let topic_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM topics WHERE id=? AND model=?)",
                params![id, active_model_hash],
                |row| row.get(0),
            )?;
            if !topic_exists {
                continue;
            }
            match topic_representative_label(
                conn,
                &id,
                active_model_hash,
                centroid.as_deref(),
                label_deadline,
            )? {
                Some(label) => {
                    let _ =
                        persist_topic_label(conn, &id, active_model_hash, &label, &mut headroom)?;
                    label
                }
                None => "Research".to_owned(),
            }
        };
        let buckets = if let Some(eligible_atom_ids) = eligible_atom_ids {
            if eligible_atom_ids.is_empty() {
                Vec::new()
            } else {
                let placeholders = vec!["?"; eligible_atom_ids.len()].join(",");
                let sql = format!(
                    "SELECT a.site,d.session,d.day,min(1.0,sum(d.mass*m.mass))
                     FROM atom_topics m
                     JOIN atoms a ON a.id=m.atom
                     JOIN atom_days d ON d.atom=a.id
                     WHERE m.topic=? AND m.atom IN ({placeholders})
                     GROUP BY a.site,d.session,d.day"
                );
                let parameters = std::iter::once(id.as_str())
                    .chain(eligible_atom_ids.iter().map(String::as_str));
                conn.prepare(&sql)?
                    .query_map(rusqlite::params_from_iter(parameters), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
        } else {
            conn.prepare("SELECT a.site,d.session,d.day,min(1.0,sum(d.mass*m.mass)) FROM atom_topics m JOIN atoms a ON a.id=m.atom JOIN atom_days d ON d.atom=a.id WHERE m.topic=? GROUP BY a.site,d.session,d.day")?.query_map([&id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,f64>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?
        };
        if buckets.is_empty() {
            continue;
        }
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

#[cfg(test)]
mod cache_algebra_tests {
    use super::*;
    use rusqlite::params;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Fraction(i128, i128);

    impl Fraction {
        fn new(numerator: i128, denominator: i128) -> Self {
            assert_ne!(denominator, 0);
            let sign = if denominator < 0 { -1 } else { 1 };
            let numerator = numerator * sign;
            let denominator = denominator.abs();
            let divisor = gcd(numerator.abs(), denominator);
            Self(numerator / divisor, denominator / divisor)
        }

        fn zero() -> Self {
            Self(0, 1)
        }

        fn one() -> Self {
            Self(1, 1)
        }

        fn add(self, other: Self) -> Self {
            Self::new(self.0 * other.1 + other.0 * self.1, self.1 * other.1)
        }

        fn mul(self, other: Self) -> Self {
            Self::new(self.0 * other.0, self.1 * other.1)
        }

        fn div(self, other: Self) -> Self {
            Self::new(self.0 * other.1, self.1 * other.0)
        }

        fn min(self, other: Self) -> Self {
            if self.0 * other.1 <= other.0 * self.1 {
                self
            } else {
                other
            }
        }

        fn text(self) -> String {
            if self.1 == 1 {
                self.0.to_string()
            } else {
                format!("{}/{}", self.0, self.1)
            }
        }
    }

    fn gcd(mut left: i128, mut right: i128) -> i128 {
        while right != 0 {
            (left, right) = (right, left % right);
        }
        left.max(1)
    }

    type Vec2 = [Fraction; 2];

    fn v(x: Fraction, y: Fraction) -> Vec2 {
        [x, y]
    }

    fn vadd(left: Vec2, right: Vec2) -> Vec2 {
        [left[0].add(right[0]), left[1].add(right[1])]
    }

    fn vscale(scale: Fraction, vector: Vec2) -> Vec2 {
        [scale.mul(vector[0]), scale.mul(vector[1])]
    }

    fn bucket_part(resultant: Vec2, mass: Fraction) -> Vec2 {
        if mass == Fraction::zero() {
            return v(Fraction::zero(), Fraction::zero());
        }
        vscale(Fraction::one().min(mass).div(mass), resultant)
    }

    fn exact_bucket_change(
        buckets: &mut BTreeMap<String, (Vec2, Fraction)>,
        cached_sum: &mut Vec2,
        key: &str,
        weight: Fraction,
        vector: Vec2,
    ) {
        let zero = Fraction::zero();
        let (resultant, mass) = buckets.get(key).copied().unwrap_or((v(zero, zero), zero));
        let before = bucket_part(resultant, mass);
        let next = (vadd(resultant, vscale(weight, vector)), mass.add(weight));
        let after = bucket_part(next.0, next.1);
        *cached_sum = vadd(
            vadd(*cached_sum, vscale(Fraction::new(-1, 1), before)),
            after,
        );
        buckets.insert(key.to_owned(), next);
    }

    #[test]
    fn exact_rational_twelve_edit_control_matches_full_recompute() {
        type BucketState = (Vec2, Fraction);
        type Active = (String, Fraction, Vec2);
        let zero = Fraction::zero();
        let one = Fraction::one();
        let q = |numerator, denominator| Fraction::new(numerator, denominator);
        let operations: [(&str, Option<&str>, Fraction, Vec2); 12] = [
            ("a", Some("x"), q(1, 4), v(one, zero)),
            ("b", Some("x"), q(3, 4), v(zero, one)),
            ("c", Some("x"), q(1, 2), v(q(3, 5), q(4, 5))),
            ("d", Some("y"), q(2, 1), v(q(-3, 5), q(4, 5))),
            ("e", Some("z"), q(1, 8), v(q(-1, 1), zero)),
            ("a", Some("x"), q(1, 2), v(one, zero)),
            ("c", None, zero, v(zero, zero)),
            ("b", None, zero, v(zero, zero)),
            ("a", None, zero, v(zero, zero)),
            ("d", Some("z"), q(2, 1), v(q(-3, 5), q(4, 5))),
            ("e", None, zero, v(zero, zero)),
            ("d", None, zero, v(zero, zero)),
        ];
        let expected = [
            ("1/4", "0"),
            ("1/4", "3/4"),
            ("11/30", "23/30"),
            ("-7/30", "47/30"),
            ("-43/120", "47/30"),
            ("-15/56", "51/35"),
            ("-13/40", "7/5"),
            ("-9/40", "4/5"),
            ("-29/40", "4/5"),
            ("-53/85", "64/85"),
            ("-3/5", "4/5"),
            ("0", "0"),
        ];
        let mut active = BTreeMap::<String, Active>::new();
        let mut buckets = BTreeMap::<String, BucketState>::new();
        let mut cached_sum = v(zero, zero);

        for (index, (atom, bucket, weight, vector)) in operations.iter().enumerate() {
            if let Some((old_bucket, old_weight, old_vector)) = active.remove(*atom) {
                exact_bucket_change(
                    &mut buckets,
                    &mut cached_sum,
                    &old_bucket,
                    q(-1, 1).mul(old_weight),
                    old_vector,
                );
            }
            if let Some(bucket) = bucket {
                active.insert((*atom).to_owned(), ((*bucket).to_owned(), *weight, *vector));
                exact_bucket_change(&mut buckets, &mut cached_sum, bucket, *weight, *vector);
            }

            let mut reference_buckets = BTreeMap::<String, BucketState>::new();
            for (bucket, weight, vector) in active.values() {
                let state = reference_buckets
                    .entry(bucket.clone())
                    .or_insert((v(zero, zero), zero));
                state.0 = vadd(state.0, vscale(*weight, *vector));
                state.1 = state.1.add(*weight);
            }
            let reference_sum = reference_buckets
                .values()
                .fold(v(zero, zero), |sum, (r, m)| vadd(sum, bucket_part(*r, *m)));
            assert_eq!(cached_sum, reference_sum, "edit {}", index + 1);
            assert_eq!(cached_sum[0].text(), expected[index].0);
            assert_eq!(cached_sum[1].text(), expected[index].1);
        }
    }

    #[test]
    fn cached_assignments_do_not_rescan_unchanged_members() {
        let directory = tempfile::tempdir().unwrap();
        let vault = crate::storage::Vault::open(&directory.path().join("cache.sqlite")).unwrap();
        let vector = {
            let mut vector = vec![0.0; algorithm::DIMENSIONS];
            vector[0] = 1.0;
            vector
        };
        for index in 0..20 {
            let atom = format!("atom-{index:02}");
            vault
                .conn
                .execute(
                    "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                    params![
                        atom,
                        "source",
                        "example.com",
                        format!("Desk lamp installation {index}"),
                        Option::<String>::None,
                        "visit",
                        format!("2026-10-{:02}T00:00:00Z", index + 1),
                        "2026-10-01T00:00:00Z",
                        1,
                        format!("canonical-{index}")
                    ],
                )
                .unwrap();
            vault
                .conn
                .execute(
                    "INSERT INTO atom_days VALUES(?,?,?,?)",
                    params![atom, "2026-10-01", "source:1:1", 1.0],
                )
                .unwrap();
            vault
                .conn
                .execute(
                    "INSERT INTO vectors VALUES(?,?,?)",
                    params![atom, "model", encode(&vector)],
                )
                .unwrap();
            REFERENCE_ROWS_READ.with(|count| count.set(0));
            DELTA_ROWS_READ.with(|count| count.set(0));
            assign(
                &vault.conn,
                &atom,
                &vector,
                "Desk lamp installation",
                "model",
            )
            .unwrap();
            let rows = REFERENCE_ROWS_READ.with(std::cell::Cell::get);
            assert_eq!(rows, 0, "cached update reread members for {atom}");
            let delta_rows = DELTA_ROWS_READ.with(std::cell::Cell::get);
            assert_eq!(
                delta_rows, 1,
                "cached update did not count its actual contribution row for {atom}"
            );
        }

        vault
            .conn
            .execute(
                "UPDATE atom_days SET mass=mass+0.25 WHERE atom='atom-00'",
                [],
            )
            .unwrap();
        REFERENCE_ROWS_READ.with(|count| count.set(0));
        DELTA_ROWS_READ.with(|count| count.set(0));
        let atom = "atom-repair";
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    atom,
                    "source",
                    "example.com",
                    "Desk lamp installation repair",
                    Option::<String>::None,
                    "visit",
                    "2026-10-30T00:00:00Z",
                    "2026-10-01T00:00:00Z",
                    1,
                    "canonical-repair"
                ],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES(?,?,?,?)",
                params![atom, "2026-10-01", "source:1:1", 1.0],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES(?,?,?)",
                params![atom, "model", encode(&vector)],
            )
            .unwrap();
        assign(
            &vault.conn,
            atom,
            &vector,
            "Desk lamp installation",
            "model",
        )
        .unwrap();
        let reread_rows = REFERENCE_ROWS_READ.with(std::cell::Cell::get);
        assert!(
            reread_rows >= 20,
            "invalidated cache did not reference-rebuild"
        );
        let delta_rows = DELTA_ROWS_READ.with(std::cell::Cell::get);
        assert_eq!(
            delta_rows, 1,
            "new atom's contribution scan was not counted"
        );
    }

    #[test]
    fn admission_boundary_uses_reference_centroid_before_decision() {
        let directory = tempfile::tempdir().unwrap();
        let vault =
            crate::storage::Vault::open(&directory.path().join("threshold.sqlite")).unwrap();
        let mut x = vec![0.0; algorithm::DIMENSIONS];
        x[0] = 1.0;
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    "anchor",
                    "source",
                    "example.com",
                    "Desk lamp installation",
                    Option::<String>::None,
                    "visit",
                    "2026-10-01T00:00:00Z",
                    "2026-10-01T00:00:00Z",
                    1,
                    "canonical-anchor"
                ],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES('anchor','2026-10-01','source:1:1',1.0)",
                [],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES('anchor','model',?)",
                [encode(&x)],
            )
            .unwrap();
        assign(&vault.conn, "anchor", &x, "Desk lamp installation", "model").unwrap();
        let topic: String = vault
            .conn
            .query_row(
                "SELECT topic FROM atom_topics WHERE atom='anchor'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let mut boundary = vec![0.0; algorithm::DIMENSIONS];
        boundary[0] = algorithm::TOPIC_ADMISSION + 5.0e-5;
        boundary[1] = (1.0_f32 - boundary[0] * boundary[0]).sqrt();
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    "boundary",
                    "source",
                    "example.com",
                    "Desk lamp installation boundary",
                    Option::<String>::None,
                    "visit",
                    "2026-10-02T00:00:00Z",
                    "2026-10-02T00:00:00Z",
                    1,
                    "canonical-boundary"
                ],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES('boundary','2026-10-02','source:1:2',1.0)",
                [],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES('boundary','model',?)",
                [encode(&boundary)],
            )
            .unwrap();

        REFERENCE_ROWS_READ.with(|count| count.set(0));
        DELTA_ROWS_READ.with(|count| count.set(0));
        assign(
            &vault.conn,
            "boundary",
            &boundary,
            "Desk lamp installation boundary",
            "model",
        )
        .unwrap();
        let reference_rows = REFERENCE_ROWS_READ.with(std::cell::Cell::get);
        assert!(
            reference_rows > 0,
            "score within the 0.62 guard skipped reference recomputation"
        );
        let selected: String = vault
            .conn
            .query_row(
                "SELECT topic FROM atom_topics WHERE atom='boundary'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            selected, topic,
            "reference decision at the admission boundary changed"
        );
    }

    #[test]
    fn runner_up_margin_guard_recomputes_both_topics() {
        let directory = tempfile::tempdir().unwrap();
        let vault = crate::storage::Vault::open(&directory.path().join("margin.sqlite")).unwrap();
        let x = {
            let mut vector = vec![0.0; algorithm::DIMENSIONS];
            vector[0] = 1.0;
            vector
        };
        let y = {
            let mut vector = vec![0.0; algorithm::DIMENSIONS];
            vector[1] = 1.0;
            vector
        };
        for (atom, vector, day, session) in [
            ("anchor-x", &x, "2026-10-01", "source:1:1"),
            ("anchor-y", &y, "2026-10-02", "source:1:2"),
        ] {
            vault
                .conn
                .execute(
                    "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                    params![
                        atom,
                        "source",
                        "example.com",
                        "Desk lamp installation",
                        Option::<String>::None,
                        "visit",
                        format!("{day}T00:00:00Z"),
                        format!("{day}T00:00:00Z"),
                        1,
                        format!("canonical-{atom}")
                    ],
                )
                .unwrap();
            vault
                .conn
                .execute(
                    "INSERT INTO atom_days VALUES(?,?,?,1.0)",
                    params![atom, day, session],
                )
                .unwrap();
            vault
                .conn
                .execute(
                    "INSERT INTO vectors VALUES(?,?,?)",
                    params![atom, "model", encode(vector)],
                )
                .unwrap();
            assign(&vault.conn, atom, vector, "Desk lamp installation", "model").unwrap();
        }
        assert_eq!(
            vault
                .conn
                .query_row("SELECT count(*) FROM topics", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );

        // For unit vectors on the coordinate axes, this construction makes
        // the leading-score gap fall just inside the 0.08 engineering guard.
        let gap = algorithm::TOPIC_MARGIN - 5.0e-5;
        let x_component = ((2.0 - gap * gap).sqrt() + gap) / 2.0;
        let mut query = vec![0.0; algorithm::DIMENSIONS];
        query[0] = x_component;
        query[1] = (1.0 - x_component * x_component).sqrt();
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    "near-margin",
                    "source",
                    "example.com",
                    "Desk lamp installation near margin",
                    Option::<String>::None,
                    "visit",
                    "2026-10-03T00:00:00Z",
                    "2026-10-03T00:00:00Z",
                    1,
                    "canonical-near-margin"
                ],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES('near-margin','2026-10-03','source:1:3',1.0)",
                [],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES('near-margin','model',?)",
                [encode(&query)],
            )
            .unwrap();

        REFERENCE_ROWS_READ.with(|count| count.set(0));
        assign(
            &vault.conn,
            "near-margin",
            &query,
            "Desk lamp installation near margin",
            "model",
        )
        .unwrap();
        let reference_rows = REFERENCE_ROWS_READ.with(std::cell::Cell::get);
        assert!(
            reference_rows >= 2,
            "near-margin guard did not refresh both candidate topics"
        );
        let memberships: i64 = vault
            .conn
            .query_row(
                "SELECT count(*) FROM atom_topics WHERE atom='near-margin'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            memberships, 2,
            "reference scores inside the margin retain soft membership"
        );
    }

    #[test]
    fn insufficient_cache_headroom_keeps_activity_reference_only() {
        let directory = tempfile::tempdir().unwrap();
        let vault = crate::storage::Vault::open(&directory.path().join("headroom.sqlite")).unwrap();
        let mut vector = vec![0.0; algorithm::DIMENSIONS];
        vector[0] = 1.0;
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    "member",
                    "source",
                    "example.com",
                    "Desk lamp installation guide",
                    Option::<String>::None,
                    "visit",
                    "2026-10-01T00:00:00Z",
                    "2026-10-01T00:00:00Z",
                    1,
                    "canonical-member"
                ],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES('member','2026-10-01','source:1:1',1.0)",
                [],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES('member','model',?)",
                [encode(&vector)],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO topics(id,label,centroid,model) VALUES('topic','Research',?,'model')",
                [vec![7u8; algorithm::DIMENSIONS * 4]],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_topics(atom,topic,mass) VALUES('member','topic',1.0)",
                [],
            )
            .unwrap();
        REFERENCE_ROWS_READ.with(|count| count.set(0));
        let activity = activity_with_cache_headroom(&vault.conn, Some("model"), 0).unwrap();
        assert_eq!(activity[0]["label"], "Desk lamp installation guide");
        assert!(REFERENCE_ROWS_READ.with(std::cell::Cell::get) > 0);
        let cache_rows: i64 = vault
            .conn
            .query_row("SELECT count(*) FROM topic_cache", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            cache_rows, 0,
            "zero headroom must not persist derived cache or label rows"
        );
        let stored_centroid: Vec<u8> = vault
            .conn
            .query_row("SELECT centroid FROM topics WHERE id='topic'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored_centroid, vec![7u8; algorithm::DIMENSIONS * 4]);
    }

    #[test]
    fn empty_topic_reassignment_with_no_cache_headroom_does_not_collide() {
        let directory = tempfile::tempdir().unwrap();
        let vault =
            crate::storage::Vault::open(&directory.path().join("empty-reassign.sqlite")).unwrap();
        let mut x = vec![0.0; algorithm::DIMENSIONS];
        x[0] = 1.0;
        let mut y = vec![0.0; algorithm::DIMENSIONS];
        y[1] = 1.0;
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    "only-member",
                    "source",
                    "example.com",
                    "Desk lamp installation",
                    Option::<String>::None,
                    "visit",
                    "2026-10-01T00:00:00Z",
                    "2026-10-01T00:00:00Z",
                    1,
                    "canonical-only-member"
                ],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES('only-member','2026-10-01','source:1:1',1.0)",
                [],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES('only-member','model',?)",
                [encode(&x)],
            )
            .unwrap();
        assign(
            &vault.conn,
            "only-member",
            &x,
            "Desk lamp installation",
            "model",
        )
        .unwrap();
        let original_topic: String = vault
            .conn
            .query_row(
                "SELECT topic FROM atom_topics WHERE atom='only-member'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        vault
            .conn
            .execute(
                "UPDATE vectors SET vector=? WHERE atom='only-member' AND model='model'",
                [encode(&y)],
            )
            .unwrap();

        assign_with_cache_headroom(
            &vault.conn,
            "only-member",
            &y,
            "Desk lamp installation",
            "model",
            0,
        )
        .unwrap();
        let topic_rows: i64 = vault
            .conn
            .query_row("SELECT count(*) FROM topics", [], |row| row.get(0))
            .unwrap();
        assert_eq!(topic_rows, 1);
        let assigned_topic: String = vault
            .conn
            .query_row(
                "SELECT topic FROM atom_topics WHERE atom='only-member'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(assigned_topic, original_topic);
        let valid: i64 = vault
            .conn
            .query_row(
                "SELECT valid FROM topic_cache WHERE topic=?",
                [&assigned_topic],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(valid, 0, "unfunded stale cache rows must remain unusable");
        assign(
            &vault.conn,
            "only-member",
            &y,
            "Desk lamp installation",
            "model",
        )
        .unwrap();
        let sum = read_valid_sum(&vault.conn, &assigned_topic, "model")
            .unwrap()
            .expect("normal headroom should rebuild the cache");
        assert!(sum[1] > 0.99);
        assert!(sum[0].abs() < 1.0e-12);
    }

    #[test]
    fn partial_headroom_empty_reassignment_does_not_cascade_stale_buckets() {
        let directory = tempfile::tempdir().unwrap();
        let vault =
            crate::storage::Vault::open(&directory.path().join("partial-empty.sqlite")).unwrap();
        let mut x = vec![0.0; algorithm::DIMENSIONS];
        x[0] = 1.0;
        let mut y = vec![0.0; algorithm::DIMENSIONS];
        y[1] = 1.0;
        vault
            .conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    "only-member",
                    "source",
                    "example.com",
                    "Desk lamp installation",
                    Option::<String>::None,
                    "visit",
                    "2026-10-01T00:00:00Z",
                    "2026-10-01T00:00:00Z",
                    1,
                    "canonical-only-member"
                ],
            )
            .unwrap();
        for index in 0..4 {
            vault
                .conn
                .execute(
                    "INSERT INTO atom_days VALUES('only-member',?,?,1.0)",
                    params!["2026-10-01", format!("source:1:{index}")],
                )
                .unwrap();
        }
        vault
            .conn
            .execute(
                "INSERT INTO vectors VALUES('only-member','model',?)",
                [encode(&x)],
            )
            .unwrap();
        assign(
            &vault.conn,
            "only-member",
            &x,
            "Desk lamp installation",
            "model",
        )
        .unwrap();
        let topic: String = vault
            .conn
            .query_row(
                "SELECT topic FROM atom_topics WHERE atom='only-member'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(topic_bucket_count(&vault.conn, &topic).unwrap(), 4);
        vault
            .conn
            .execute(
                "UPDATE vectors SET vector=? WHERE atom='only-member' AND model='model'",
                [encode(&y)],
            )
            .unwrap();

        // Four pages fund the centroid refresh and leave two pages: enough to
        // create an empty accumulator, but far below the ten-page cascade cost
        // for this topic's stale accumulator plus four bucket/index rows.
        let partial_headroom = page_write_bytes(&vault.conn, 4).unwrap();
        assign_with_cache_headroom(
            &vault.conn,
            "only-member",
            &y,
            "Desk lamp installation",
            "model",
            partial_headroom,
        )
        .unwrap();
        assert_eq!(topic_bucket_count(&vault.conn, &topic).unwrap(), 4);
        let valid: i64 = vault
            .conn
            .query_row(
                "SELECT valid FROM topic_cache WHERE topic=?",
                [&topic],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(valid, 0, "underfunded cache rows must remain invalid");

        assign(
            &vault.conn,
            "only-member",
            &y,
            "Desk lamp installation",
            "model",
        )
        .unwrap();
        assert_eq!(topic_bucket_count(&vault.conn, &topic).unwrap(), 4);
        assert!(read_valid_sum(&vault.conn, &topic, "model")
            .unwrap()
            .is_some());
    }

    #[test]
    fn global_cache_eviction_is_ascending_and_respects_the_bucket_limit() {
        let directory = tempfile::tempdir().unwrap();
        let vault = crate::storage::Vault::open(&directory.path().join("eviction.sqlite")).unwrap();
        let zero_resultant = encode_f64(&vec![0.0; algorithm::DIMENSIONS]);
        for topic in ["a-topic", "b-topic", "z-topic"] {
            vault
                .conn
                .execute(
                    "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
                    params![
                        topic,
                        topic,
                        encode(&vec![0.0; algorithm::DIMENSIONS]),
                        "model"
                    ],
                )
                .unwrap();
        }
        for topic in ["a-topic", "b-topic"] {
            vault.conn.execute(
                "INSERT INTO topic_cache(topic,model,valid,sum,sum_hash,dirty_label) VALUES(?,'model',1,?,?,0)",
                params![topic, ZERO_SUM, cache_sum_hash("model", ZERO_SUM)],
            ).unwrap();
            for index in 0..2048 {
                let key = ("site.example".to_owned(), format!("session-{index:04}"));
                let hash = cache_bucket_hash(topic, "model", &key, 1.0, &zero_resultant);
                vault.conn.execute(
                    "INSERT INTO topic_cache_buckets(topic,site,session,mass,resultant,resultant_hash) VALUES(?,?,?,1.0,?,?)",
                    params![topic, key.0, key.1, zero_resultant, hash],
                ).unwrap();
            }
        }
        assert_eq!(cache_bucket_count(&vault.conn).unwrap(), CACHE_BUCKET_LIMIT);
        assert_eq!(
            cache_eviction_plan(&vault.conn, "z-topic", CACHE_BUCKET_LIMIT + 1).unwrap(),
            Some(vec!["a-topic".to_owned()]),
        );

        let mut sum = vec![0.0; algorithm::DIMENSIONS];
        sum[0] = 1.0;
        let mut buckets = BTreeMap::new();
        buckets.insert(
            ("new.example".to_owned(), "new-session".to_owned()),
            Bucket {
                mass: 1.0,
                resultant: sum.clone(),
            },
        );
        let mut headroom = u64::MAX;
        assert!(store_reference_cache(
            &vault.conn,
            "z-topic",
            "model",
            &sum,
            &buckets,
            &mut headroom,
        )
        .unwrap());
        assert_eq!(cache_bucket_count(&vault.conn).unwrap(), 2049);
        for (topic, expected) in [("a-topic", false), ("b-topic", true), ("z-topic", true)] {
            let exists: bool = vault
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM topic_cache WHERE topic=?)",
                    [topic],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, expected, "unexpected cache eviction for {topic}");
        }
    }
}
