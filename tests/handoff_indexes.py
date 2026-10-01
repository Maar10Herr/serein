#!/usr/bin/env python3
"""Populated schema-4 SQLite probe for T01 targeted-index acceptance.

This standard-library harness creates two isolated copies of the same bounded
synthetic fixture. Both copies use the schema-4 feedback sequence; the only
difference is whether the three T01 secondary indexes exist during writes and
queries. It records query plans, database/WAL bytes, SQLite insert timings and
an ordered ID/text/state projection. It never opens a user vault or profile.
"""

import argparse
import datetime as dt
import hashlib
import json
import math
import os
import pathlib
import platform
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]
ATOM_COUNT = 10_000
TOPIC_COUNT = 128
SITE_COUNT = 97
SAMPLES = 5
SOURCE_A = "10000000-0000-4000-8000-000000000001"
SOURCE_B = "10000000-0000-4000-8000-000000000002"
MODEL = "synthetic-model-hash"
CONFIRMATION_LIMIT = 1_000
PACKET_LIMIT = 6
PER_SITE_LIMIT = 2
BASE_TIME = dt.datetime(2026, 7, 2, tzinfo=dt.timezone.utc)
FIXED_FEEDBACK_TIME = "2026-10-01T12:34:56Z"

INDEX_DDL = (
    "CREATE INDEX atoms_by_source_recency ON atoms(source,last_seen DESC,id)",
    "CREATE INDEX memberships_by_topic ON atom_topics(topic,atom)",
    "CREATE INDEX feedback_by_atom_action ON feedback(atom,action,seq)",
)


def atom_id(index):
    return f"{index + 1:08x}-0000-4000-8000-{index + 1:012x}"


INDEX_PROBES = {
    "atoms_by_source_recency": (
        "SELECT id,site,title,query,last_seen FROM atoms "
        "WHERE source=? ORDER BY last_seen DESC,id LIMIT 10000",
        (SOURCE_A,),
    ),
    "memberships_by_topic": (
        "SELECT atom,topic,mass FROM atom_topics WHERE topic=? ORDER BY atom",
        ("topic-000",),
    ),
    "feedback_by_atom_action": (
        "SELECT text FROM feedback WHERE atom=? AND action=? ORDER BY seq DESC LIMIT 1",
        (atom_id(100), "confirm_constraint"),
    ),
}


def synthetic_rows():
    atoms = []
    events = []
    days = []
    feedback = []
    memberships = []
    source_count = 0
    confirmation_count = 0
    event_base = 20_000
    feedback_base = 40_000

    for index in range(ATOM_COUNT):
        atom = atom_id(index)
        source = SOURCE_B if index % 10 == 0 else SOURCE_A
        source_count += source == SOURCE_A
        site_index = index % SITE_COUNT
        site = f"site-{site_index:03d}.example.org"
        topic_index = index % TOPIC_COUNT
        title = f"Research topic {topic_index:03d} item {index:05d}"
        query = f"topic {topic_index:03d} reference item {index:05d}"
        # Pin three confirmed and three observed source-A items at the packet
        # head so the equality check covers both states on every run.
        packet_head_offsets = {
            0: 400,
            1: 300,
            21: 250,
            41: 200,
            2: 150,
            3: 100,
            6: 50,
        }
        offset = packet_head_offsets.get(
            index,
            (index * 7_919) % (88 * 86_400),
        )
        if index in packet_head_offsets:
            offset += 89 * 86_400
        last_seen = BASE_TIME + dt.timedelta(seconds=offset)
        time_text = last_seen.isoformat(timespec="seconds").replace("+00:00", "Z")
        atoms.append(
            (atom, source, site, title, query, "search", time_text, time_text, 30, f"canonical-{index:05d}")
        )
        events.append(
            (
                source,
                atom_id(event_base + index),
                atom,
                atom_id(event_base + ATOM_COUNT + index),
                time_text,
            )
        )
        days.append((atom, last_seen.date().isoformat(), f"window-{index // 5_000}", 1.0))
        memberships.append((atom, f"topic-{topic_index:03d}", 1.0))
        if index % 4 == 0:
            memberships.append((atom, f"topic-{(topic_index + 1) % TOPIC_COUNT:03d}", 0.25))

        if confirmation_count < CONFIRMATION_LIMIT and index % 20 in (0, 1):
            action = "confirm_constraint"
            text = f"Research item {index:05d} has explicit confirmed detail {index % 17}."
            confirmation_count += 1
        else:
            action = ("do_not_use", "wrong_topic", "temporary_research", "not_about_me")[index % 4]
            text = None
        feedback.append(
            (feedback_base + index, atom, action, text, FIXED_FEEDBACK_TIME)
        )
        if index % 97 == 0 and action != "confirm_constraint":
            feedback.append(
                (
                    feedback_base + ATOM_COUNT + index,
                    atom,
                    "temporary_research",
                    None,
                    FIXED_FEEDBACK_TIME,
                )
            )

    return {
        "atoms": atoms,
        "events": events,
        "atom_days": days,
        "feedback": feedback,
        "atom_topics": memberships,
        "source_a_atoms": source_count,
        "confirmations": confirmation_count,
    }


def create_schema(conn, indexed):
    conn.execute("PRAGMA foreign_keys=ON")
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=FULL")
    conn.execute("PRAGMA wal_autocheckpoint=0")
    conn.executescript(
        """
        CREATE TABLE sources(id TEXT PRIMARY KEY,policy TEXT NOT NULL);
        CREATE TABLE atoms(
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            site TEXT NOT NULL,
            title TEXT NOT NULL,
            query TEXT,
            kind TEXT NOT NULL,
            first_seen TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            seconds INTEGER NOT NULL,
            canonical TEXT NOT NULL UNIQUE
        );
        CREATE TABLE events(
            source TEXT NOT NULL,
            id TEXT NOT NULL,
            atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
            visit TEXT NOT NULL,
            time TEXT NOT NULL,
            PRIMARY KEY(source,id)
        );
        CREATE TABLE atom_days(
            atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
            day TEXT NOT NULL,
            session TEXT NOT NULL,
            mass REAL NOT NULL,
            PRIMARY KEY(atom,day,session)
        );
        CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query,tokenize='unicode61');
        CREATE TRIGGER atom_insert AFTER INSERT ON atoms BEGIN
            INSERT INTO atom_fts(id,title,query) VALUES(new.id,new.title,new.query);
        END;
        CREATE TABLE feedback(
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
            action TEXT NOT NULL,
            text TEXT,
            time TEXT NOT NULL
        );
        CREATE TABLE topics(id TEXT PRIMARY KEY,label TEXT NOT NULL,centroid BLOB NOT NULL,model TEXT NOT NULL);
        CREATE TABLE atom_topics(
            atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
            topic TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
            mass REAL NOT NULL,
            PRIMARY KEY(atom,topic)
        );
        """
    )
    conn.executemany(
        "INSERT INTO sources(id,policy) VALUES(?,?)",
        [(SOURCE_A, "{}"), (SOURCE_B, "{}")],
    )
    conn.executemany(
        "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
        [
            (f"topic-{index:03d}", f"Synthetic topic {index:03d}", b"x" * 1_024, MODEL)
            for index in range(TOPIC_COUNT)
        ],
    )
    if indexed:
        conn.executescript(";".join(INDEX_DDL) + ";")
    conn.commit()


def populate(conn, fixture):
    started = time.perf_counter()
    conn.execute("BEGIN IMMEDIATE")
    conn.executemany(
        "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)", fixture["atoms"]
    )
    conn.executemany("INSERT INTO events VALUES(?,?,?,?,?)", fixture["events"])
    conn.executemany("INSERT INTO atom_days VALUES(?,?,?,?)", fixture["atom_days"])
    conn.executemany(
        "INSERT INTO feedback(id,atom,action,text,time) VALUES(?,?,?,?,?)",
        [
            (f"{row_id:08x}-0000-4000-8000-{row_id:012x}", atom, action, text, stamp)
            for row_id, atom, action, text, stamp in fixture["feedback"]
        ],
    )
    conn.executemany(
        "INSERT INTO atom_topics VALUES(?,?,?)", fixture["atom_topics"]
    )
    conn.commit()
    return (time.perf_counter() - started) * 1_000


def explain(conn, sql, params):
    return [row[3] for row in conn.execute("EXPLAIN QUERY PLAN " + sql, params)]


def projected_packet(conn, source):
    candidates = conn.execute(
        "SELECT id,site,title,query,last_seen FROM atoms "
        "WHERE source=? ORDER BY last_seen DESC,id LIMIT 10000",
        (source,),
    )
    per_site = {}
    packet = []
    for atom, site, title, query, last_seen in candidates:
        suppressed = conn.execute(
            "SELECT EXISTS(SELECT 1 FROM feedback WHERE atom=? AND action IN ('do_not_use','wrong_topic'))",
            (atom,),
        ).fetchone()[0]
        if suppressed:
            continue
        count = per_site.get(site, 0)
        if count >= PER_SITE_LIMIT:
            continue
        confirmation = conn.execute(
            "SELECT text FROM feedback WHERE atom=? AND action='confirm_constraint' "
            "ORDER BY seq DESC LIMIT 1",
            (atom,),
        ).fetchone()
        state = "confirmed" if confirmation else "observed"
        text = confirmation[0] if confirmation else (query or title)
        packet.append(
            {"id": atom, "site": site, "text": text, "state": state, "last_seen": last_seen}
        )
        per_site[site] = count + 1
        if len(packet) >= PACKET_LIMIT:
            break
    return packet


def file_bytes(path):
    return path.stat().st_size if path.exists() else 0


def run_variant(directory, fixture, indexed):
    path = directory / ("indexed.sqlite" if indexed else "unindexed.sqlite")
    conn = sqlite3.connect(path)
    create_schema(conn, indexed)
    ingest_ms = populate(conn, fixture)
    conn.execute("ANALYZE")
    conn.commit()

    plans = {
        name: explain(conn, sql, params)
        for name, (sql, params) in INDEX_PROBES.items()
    }
    packets = {
        source: projected_packet(conn, source) for source in (SOURCE_A, SOURCE_B)
    }
    target_topic = conn.execute(
        "SELECT atom,topic,mass FROM atom_topics WHERE topic=? ORDER BY atom",
        ("topic-000",),
    ).fetchall()
    membership_digest = [list(row) for row in target_topic[:8]] + [len(target_topic)]
    db_path = pathlib.Path(str(path))
    wal_path = pathlib.Path(str(path) + "-wal")
    sizes = {
        "database_bytes": file_bytes(db_path),
        "wal_bytes": file_bytes(wal_path),
        "database_plus_wal_bytes": file_bytes(db_path) + file_bytes(wal_path),
        "page_count": conn.execute("PRAGMA page_count").fetchone()[0],
        "page_size": conn.execute("PRAGMA page_size").fetchone()[0],
    }
    conn.close()
    return {
        "indexed": indexed,
        "ingest_ms": round(ingest_ms, 2),
        "plans": plans,
        "packets": packets,
        "membership_probe": membership_digest,
        "sizes": sizes,
    }


def distribution(values):
    ordered = sorted(values)
    return {
        "samples": len(values),
        "values_ms": [round(value, 2) for value in values],
        "median_ms": round(statistics.median(values), 2),
        "p95_ms": round(ordered[math.ceil(0.95 * len(ordered)) - 1], 2),
    }


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def source_fingerprint():
    relative_paths = (
        "Cargo.lock",
        "crates/serein-core/src/storage.rs",
        "crates/serein-core/src/retrieval.rs",
        "crates/serein-core/src/feedback.rs",
        "crates/serein-core/tests/feedback_sequence.rs",
        "tests/handoff_indexes.py",
    )
    digest = hashlib.sha256()
    for relative in relative_paths:
        path = ROOT / relative
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return {
        "files": list(relative_paths),
        "combined_sha256": digest.hexdigest(),
        "cargo_lock_sha256": sha256((ROOT / "Cargo.lock").read_bytes()),
    }


def run(args):
    fixture = synthetic_rows()
    fixture_sha256 = sha256(
        json.dumps(fixture, sort_keys=True, separators=(",", ":")).encode("utf-8")
    )
    timings = {"unindexed": [], "indexed": []}
    sizes = {"unindexed": [], "indexed": []}
    first_results = {}

    with tempfile.TemporaryDirectory(prefix="serein-t01-indexes-") as root:
        for sample in range(SAMPLES):
            pair_dir = pathlib.Path(root) / f"pair-{sample:02d}"
            pair_dir.mkdir()
            order = (False, True) if sample % 2 == 0 else (True, False)
            results = {}
            for indexed in order:
                result = run_variant(pair_dir, fixture, indexed)
                label = "indexed" if indexed else "unindexed"
                timings[label].append(result["ingest_ms"])
                sizes[label].append(result["sizes"])
                results[label] = result
            assert results["unindexed"]["packets"] == results["indexed"]["packets"], (
                "secondary indexes changed ordered output IDs/text/states"
            )
            assert results["unindexed"]["membership_probe"] == results["indexed"]["membership_probe"]
            if sample == 0:
                first_results = results

    for index_name in INDEX_PROBES:
        before = "\n".join(first_results["unindexed"]["plans"][index_name])
        after = "\n".join(first_results["indexed"]["plans"][index_name])
        assert index_name not in before, f"{index_name} unexpectedly exists in the unindexed fixture"
        assert index_name in after, f"populated query did not use {index_name}: {after}"

    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        commit = "unavailable"

    result = {
        "gate": "P01",
        "status": "PASS",
        "source_commit": commit,
        "source_tree_fingerprint": source_fingerprint(),
        "working_tree_changes_included": True,
        "platform": platform.platform(),
        "architecture": platform.machine(),
        "python": sys.version.split()[0],
        "sqlite": sqlite3.sqlite_version,
        "fixture": {
            "atoms": ATOM_COUNT,
            "source_a_atoms": fixture["source_a_atoms"],
            "sources": 2,
            "sites": SITE_COUNT,
            "topics": TOPIC_COUNT,
            "confirmations": fixture["confirmations"],
            "feedback_rows": len(fixture["feedback"]),
            "membership_rows": len(fixture["atom_topics"]),
            "events": len(fixture["events"]),
            "sha256": fixture_sha256,
            "database_schema": "schema-4 logical layout; only the three T01 secondary indexes vary",
            "scope": "deterministic generated rows; SQLite write transaction only, not end-to-end native ingestion",
        },
        "samples": SAMPLES,
        "query_plans": {
            "without_targeted_indexes": first_results["unindexed"]["plans"],
            "with_targeted_indexes": first_results["indexed"]["plans"],
        },
        "ordered_packets": first_results["indexed"]["packets"],
        "index_output_equal": True,
        "membership_probe_equal": True,
        "ingest_timings": {
            name: distribution(values) for name, values in timings.items()
        },
        "database_and_wal_sizes": sizes,
        "limitations": [
            "This is a populated SQLite access-path and write-cost probe, not a product-level latency claim.",
            "The fixture preserves feedback sequence order in both variants; migration correctness is covered by feedback_sequence.rs.",
        ],
    }
    rendered = json.dumps(result, indent=2)
    if args.output:
        output_path = pathlib.Path(args.output).expanduser().resolve()
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(rendered + "\n", encoding="utf-8")
        print(f"Wrote {output_path}")
    else:
        print(rendered)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        help="write the JSON result to this path; default is stdout",
    )
    run(parser.parse_args())


if __name__ == "__main__":
    main()
