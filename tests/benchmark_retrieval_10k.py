#!/usr/bin/env python3
"""Measure process-level recall with a full 10,000-atom synthetic vault.

The harness creates an isolated Serein data directory, ingests only generated
browser metadata, builds the local semantic index, and measures end-to-end
recall calls including process startup. It never reads a browser profile.
"""
import datetime
from contextlib import closing
import itertools
import json
import math
import os
import pathlib
import platform
import sqlite3
import statistics
import struct
import subprocess
import tempfile
import time
import uuid
import sys
try:
    import resource
except ImportError:
    resource = None

ROOT = pathlib.Path(__file__).resolve().parents[1]
ATOM_COUNT = int(os.environ.get("SEREIN_BENCH_ATOMS", "10000"))
BATCH_SIZE = 32  # Native protocol limit.
SAMPLES = int(os.environ.get("SEREIN_BENCH_SAMPLES", "30"))
INCREMENTAL_BATCHES = int(os.environ.get("SEREIN_BENCH_INCREMENTAL_BATCHES", "0"))
FIXTURE_SEED = os.environ.get("SEREIN_BENCH_SEED", "capacity-fixture-v1")
UID_SEQUENCE = itertools.count()
QUERY = "mechanical keyboard switch durability"
FACETS = ["tactile switch lifespan"]
CATEGORIES = [
    "mechanical keyboard switch",
    "wireless headphones",
    "ergonomic desk chair",
    "compact espresso machine",
    "robot vacuum mapping",
    "ultrawide monitor",
    "trail running shoes",
    "HEPA air purifier",
    "mirrorless camera autofocus",
    "induction cookware",
]
if os.environ.get("SEREIN_BENCH_CORPUS") == "growing":
    CATEGORIES = ["mechanical keyboard switch"]


def uid():
    return str(uuid.uuid5(uuid.NAMESPACE_URL, f"serein-benchmark:{FIXTURE_SEED}:{next(UID_SEQUENCE)}"))


def stats(values):
    ordered = sorted(values)
    return {
        "samples": len(values),
        "median_ms": round(statistics.median(values), 2),
        "p95_ms": round(ordered[math.ceil(0.95 * len(ordered)) - 1], 2),
        "min_ms": round(min(values), 2),
        "max_ms": round(max(values), 2),
    }


def run_json(command, *, env, args=(), request=None, timeout=10):
    payload = json.dumps(request).encode() if request is not None else None
    result = subprocess.run(
        [str(command), *args, "--json"],
        input=payload,
        capture_output=True,
        env=env,
        timeout=timeout,
        check=True,
    )
    return json.loads(result.stdout)


def normalize_unindexed_atom_ids(database_path):
    """Give newly ingested synthetic atoms seed-stable IDs before refresh.

    This benchmark-only normalization keeps traversal order identical across
    schema binaries while preserving every already-indexed atom ID. It runs
    only against the harness's isolated temporary vault.
    """
    with closing(sqlite3.connect(database_path)) as database:
        database.execute("PRAGMA foreign_keys=OFF")
        try:
            database.execute("BEGIN IMMEDIATE")
            all_rows = database.execute(
                "SELECT id,source,site,kind,title,query FROM atoms ORDER BY id"
            ).fetchall()
            all_ids = {row[0] for row in all_rows}
            pending_rows = database.execute(
                "SELECT a.id,a.source,a.site,a.kind,a.title,a.query FROM atoms a "
                "WHERE NOT EXISTS (SELECT 1 FROM vectors v WHERE v.atom=a.id) "
                "ORDER BY a.id"
            ).fetchall()
            if not pending_rows:
                database.commit()
                return 0

            already_indexed = database.execute(
                "SELECT EXISTS(SELECT 1 FROM vectors)"
            ).fetchone()[0]
            if not already_indexed:
                for table in (
                    "topics",
                    "atom_topics",
                    "topic_skips",
                    "feedback",
                    "topic_cache",
                    "topic_cache_buckets",
                ):
                    if table not in {
                        row[0]
                        for row in database.execute(
                            "SELECT name FROM sqlite_master WHERE type='table'"
                        )
                    }:
                        continue
                    count = database.execute(
                        f"SELECT count(*) FROM {table}"
                    ).fetchone()[0]
                    if count:
                        raise RuntimeError(
                            f"Cannot normalize initial atom IDs after {table} was populated."
                        )

            mapping = {}
            for old_id, source, site, kind, title, query in pending_rows:
                name = "\0".join(
                    (FIXTURE_SEED, source, site, kind, title, query or "")
                )
                mapping[old_id] = str(
                    uuid.uuid5(uuid.NAMESPACE_URL, f"serein-benchmark-atom:{name}")
                )
            targets = list(mapping.values())
            if len(set(targets)) != len(targets):
                raise RuntimeError("Deterministic benchmark atom IDs collided.")
            if set(targets) & (all_ids - set(mapping)):
                raise RuntimeError("A deterministic benchmark atom ID matches an indexed atom.")

            pending_ids = tuple(mapping)
            placeholders = ",".join("?" for _ in pending_ids)
            for table in ("vectors", "atom_topics", "topic_skips", "feedback"):
                count = database.execute(
                    f"SELECT count(*) FROM {table} WHERE atom IN ({placeholders})",
                    pending_ids,
                ).fetchone()[0]
                if count:
                    raise RuntimeError(
                        f"Cannot normalize benchmark atom IDs after {table} was populated."
                    )

            reserved = all_ids | set(targets)
            temporary = {}
            for old_id in mapping:
                temp_id = str(uuid.uuid4())
                while temp_id in reserved:
                    temp_id = str(uuid.uuid4())
                reserved.add(temp_id)
                temporary[old_id] = temp_id

            def move_atom(old_id, new_id):
                database.execute("UPDATE events SET atom=? WHERE atom=?", (new_id, old_id))
                database.execute("UPDATE atom_days SET atom=? WHERE atom=?", (new_id, old_id))
                database.execute("UPDATE atom_fts SET id=? WHERE id=?", (new_id, old_id))
                database.execute("UPDATE atoms SET id=? WHERE id=?", (new_id, old_id))

            for old_id, temp_id in temporary.items():
                move_atom(old_id, temp_id)
            for old_id, new_id in mapping.items():
                move_atom(temporary[old_id], new_id)

            violations = database.execute("PRAGMA foreign_key_check").fetchall()
            if violations:
                raise RuntimeError(
                    f"Foreign-key violations after deterministic IDs: {violations[:3]}"
                )
            database.commit()
            database.execute("PRAGMA foreign_keys=ON")
            if database.execute("PRAGMA foreign_keys").fetchone()[0] != 1:
                raise RuntimeError("Could not restore SQLite foreign-key enforcement.")
            violations = database.execute("PRAGMA foreign_key_check").fetchall()
            if violations:
                raise RuntimeError(
                    f"Foreign-key violations after ID normalization: {violations[:3]}"
                )
            return len(mapping)
        except Exception:
            if database.in_transaction:
                database.rollback()
            raise
        finally:
            database.execute("PRAGMA foreign_keys=ON")


def main():
    if not 1 <= ATOM_COUNT <= 10000 or SAMPLES < 1 or INCREMENTAL_BATCHES < 0:
        raise SystemExit("Invalid benchmark bounds.")
    if ATOM_COUNT + BATCH_SIZE * INCREMENTAL_BATCHES > 10000:
        raise SystemExit("Incremental measurements must stay below the atom cap.")
    cli = pathlib.Path(os.environ.get("SEREIN_BENCH_CLI", ROOT / "target/release/serein"))
    host = pathlib.Path(os.environ.get("SEREIN_BENCH_HOST", ROOT / "target/release/serein-host"))
    if not cli.is_file() or not host.is_file():
        raise SystemExit("Build the release binaries first: cargo build --release --offline")

    started = time.perf_counter()
    with tempfile.TemporaryDirectory(
        prefix="serein-benchmark-10k-",
        dir=str(pathlib.Path(tempfile.gettempdir()).resolve()),
    ) as tmp:
        env = {
            **os.environ,
            "SEREIN_DATA_DIR": str(pathlib.Path(tmp) / "data"),
            "SEREIN_INSTALL_HOME": str(pathlib.Path(tmp) / "install"),
        }
        extension_id = json.loads(
            (ROOT / "release/extension-identity.json").read_text()
        )["chrome_extension_id"]
        source = uid()
        nonce = uid() + uid()
        setup = {
            "protocol": 1,
            "source_id": source,
            "extension_id": extension_id,
            "browser": "chrome",
            "nonce": nonce,
            "expires_at": int(time.time()) + 890,
            "adapters": ["generic"],
            "label": "Synthetic 10k benchmark",
            "consent": True,
        }
        connection = run_json(cli, env=env, args=("setup", "--request-stdin"), request=setup, timeout=15)
        database_path = pathlib.Path(connection["database_path"])

        def storage_snapshot():
            with closing(sqlite3.connect(database_path)) as database:
                tables = {row[0] for row in database.execute("SELECT name FROM sqlite_master WHERE type='table'")}
                counts = {
                    table: database.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                    for table in ("atoms", "vectors", "topics", "atom_topics", "topic_cache", "topic_cache_buckets")
                    if table in tables
                }
            sizes = {
                name: file.stat().st_size if file.exists() else 0
                for name, file in (("database", database_path), ("wal", pathlib.Path(str(database_path) + "-wal")), ("shm", pathlib.Path(str(database_path) + "-shm")))
            }
            return {"rows": counts, "bytes": sizes, "database_and_wal_bytes": sizes["database"] + sizes["wal"]}

        def native(op, payload):
            request = {
                "protocol": 1,
                "request_id": uid(),
                "source_id": source,
                "op": op,
                "capture_epoch": 2,
                "payload": payload,
            }
            frame = json.dumps(request).encode()
            result = subprocess.run(
                [str(host), f"chrome-extension://{extension_id}/"],
                input=struct.pack("=I", len(frame)) + frame,
                capture_output=True,
                env=env,
                timeout=10,
                check=True,
            )
            size = struct.unpack("=I", result.stdout[:4])[0]
            if size != len(result.stdout) - 4:
                raise RuntimeError("Native host returned a malformed frame.")
            return json.loads(result.stdout[4:])

        hello = native("hello", {"nonce": nonce})
        if hello.get("status") != "ok":
            raise RuntimeError("Synthetic benchmark source could not be paired.")
        policy = {
            "consent": True,
            "paused": False,
            "recall_enabled": True,
            "selected_only": False,
            "selected_sites": [],
            "excluded_sites": [],
            "capture_epoch": 2,
        }
        native("policy.update", policy)

        # Ten distinct shopping/research categories approximate a varied
        # metadata vault. Every tenth record belongs to the measured query.
        ingest_started = time.perf_counter()
        all_events = []
        now = os.environ.get("SEREIN_BENCH_TIME") or datetime.datetime.now(datetime.timezone.utc).isoformat()
        for i in range(ATOM_COUNT):
            category = CATEGORIES[i % len(CATEGORIES)]
            serial = i // len(CATEGORIES)
            detail = [
                "review comparison",
                "long term reliability",
                "price and specifications",
                "owner experience",
                "replacement parts",
            ][serial % 5]
            all_events.append(
                {
                    "event_id": uid(),
                    "visit_id": uid(),
                    "site_key": f"shop{(i // 10) % 20:02d}.example.org",
                    "site_epoch": 0,
                    "observed_at": now,
                    "kind": "search",
                    "title": f"{category} option {serial:04d} review specifications",
                    "search_query": f"best {category} {detail} item {serial:04d}",
                    "foreground_seconds": 30,
                }
            )
        for offset in range(0, ATOM_COUNT, BATCH_SIZE):
            events = all_events[offset : offset + BATCH_SIZE]
            response = native("ingest", {"events": events})
            if len(response.get("acknowledged_ids", [])) != len(events):
                raise RuntimeError(
                    f"Ingest accepted {len(response.get('acknowledged_ids', []))} "
                    f"of {len(events)} generated events."
                )
        ingest_ms = (time.perf_counter() - ingest_started) * 1000
        del all_events
        deterministic_atom_ids = os.environ.get("SEREIN_BENCH_DETERMINISTIC_IDS") == "1"
        if deterministic_atom_ids:
            normalized = normalize_unindexed_atom_ids(database_path)
            if normalized != ATOM_COUNT:
                raise RuntimeError(
                    f"Normalized {normalized} of {ATOM_COUNT} initial atom IDs."
                )

        before_index = native("status", {})
        after_ingest_storage = storage_snapshot()
        if before_index.get("atoms") != ATOM_COUNT:
            raise RuntimeError(
                f"Expected exactly {ATOM_COUNT} synthetic atoms, found "
                f"{before_index.get('atoms')} after ingestion."
            )

        indexing_started = time.perf_counter()
        refresh_calls = 0
        refresh_samples = []
        while True:
            call_started = time.perf_counter()
            response = subprocess.run(
                [str(cli), "refresh", "--budget-ms", "5000", "--json"],
                capture_output=True,
                env=env,
                timeout=15,
                check=True,
            )
            indexed = json.loads(response.stdout)
            refresh_samples.append({"elapsed_ms": (time.perf_counter() - call_started) * 1000, "processed": indexed.get("processed"), "pending_atoms": indexed.get("pending_atoms")})
            refresh_calls += 1
            if indexed.get("pending_atoms") == 0:
                break
            if refresh_calls >= 100:
                raise RuntimeError(
                    f"Index remained incomplete after {refresh_calls} calls: {indexed}"
                )
        indexing_ms = (time.perf_counter() - indexing_started) * 1000
        after_backfill_storage = storage_snapshot()
        if os.environ.get("SEREIN_BENCH_CORPUS") == "growing" and after_backfill_storage["rows"].get("topics") != 1:
            raise RuntimeError("The growing-topic fixture did not form exactly one topic.")
        incremental = []
        for batch in range(INCREMENTAL_BATCHES):
            events = [
                {
                    "event_id": uid(), "visit_id": uid(),
                    "site_key": "incremental.example.org",
                    "site_epoch": 0,
                    "observed_at": now,
                    "kind": "search",
                    "title": f"{CATEGORIES[i % len(CATEGORIES)]} incremental example {batch * BATCH_SIZE + i:04d}",
                    "search_query": f"{CATEGORIES[i % len(CATEGORIES)]} incremental example {batch * BATCH_SIZE + i:04d}",
                    "foreground_seconds": 30,
                }
                for i in range(BATCH_SIZE)
            ]
            call_started = time.perf_counter()
            ingested = native("ingest", {"events": events})
            incremental_ingest_ms = (time.perf_counter() - call_started) * 1000
            if len(ingested.get("acknowledged_ids", [])) != BATCH_SIZE:
                raise RuntimeError("An incremental batch was not fully acknowledged.")
            if deterministic_atom_ids:
                normalized = normalize_unindexed_atom_ids(database_path)
                if normalized != BATCH_SIZE:
                    raise RuntimeError(
                        f"Normalized {normalized} of {BATCH_SIZE} incremental atom IDs."
                    )
            batch_refreshes = []
            for _ in range(100):
                call_started = time.perf_counter()
                indexed = run_json(cli, env=env, args=("refresh", "--budget-ms", "5000"), timeout=15)
                batch_refreshes.append({"elapsed_ms": (time.perf_counter() - call_started) * 1000, "processed": indexed.get("processed"), "pending_atoms": indexed.get("pending_atoms")})
                if indexed.get("pending_atoms") == 0:
                    break
            else:
                raise RuntimeError("Incremental refresh did not finish.")
            incremental.append({"ingest_ms": incremental_ingest_ms, "refresh_calls": batch_refreshes, "total_refresh_ms": sum(call["elapsed_ms"] for call in batch_refreshes), "storage": storage_snapshot()})

        # With the index complete, refresh measures process startup, SQLite
        # open, and model-pack initialization without embedding any atoms.
        no_op_refresh = []
        for _ in range(10):
            call_started = time.perf_counter()
            response = subprocess.run(
                [str(cli), "refresh", "--budget-ms", "1000", "--json"],
                capture_output=True,
                env=env,
                timeout=5,
                check=True,
            )
            no_op_refresh.append((time.perf_counter() - call_started) * 1000)
            packet = json.loads(response.stdout)
            if packet.get("pending_atoms") != 0 or packet.get("processed") != 0:
                raise RuntimeError("Expected the measured refresh to do no indexing work.")

        request = {
            "protocol": 1,
            "request_id": uid(),
            "client": "generic",
            "vault": "default",
            "query": QUERY,
            "facets": FACETS,
            "scope": ["research"],
            "max_bytes": 16_384,
            "budget_ms": 1500,
        }
        # One warm-up call makes the reported sample set consistent with the
        # warm local cache used for repeat assistant invocations.
        run_json(
            cli,
            env=env,
            args=("recall", "--request-stdin"),
            request=request,
            timeout=5,
        )
        def recall_samples(count, expected_mode):
            durations = []
            packets = []
            for _ in range(count):
                request["request_id"] = uid()
                payload = json.dumps(request).encode()
                call_started = time.perf_counter()
                result = subprocess.run(
                    [str(cli), "recall", "--request-stdin", "--json"],
                    input=payload,
                    capture_output=True,
                    env=env,
                    timeout=5,
                    check=True,
                )
                durations.append((time.perf_counter() - call_started) * 1000)
                packet = json.loads(result.stdout)
                if not packet.get("context"):
                    raise RuntimeError("The target query returned no synthetic evidence.")
                if len(result.stdout) > request["max_bytes"]:
                    raise RuntimeError("Recall exceeded its response byte budget.")
                if packet.get("index", {}).get("mode") != expected_mode:
                    raise RuntimeError(
                        f"Expected {expected_mode} mode, got {packet.get('index', {}).get('mode')}."
                    )
                packets.append(packet)
            return durations, packets

        recalls, final_packets = recall_samples(SAMPLES, "hybrid")

        # Temporarily hide only the synthetic vault's copied model manifest to
        # compare the same corpus under its explicit lexical fallback. Restore
        # it in all cases before the temporary directory is removed.
        manifest = pathlib.Path(env["SEREIN_DATA_DIR"]) / "models/current/manifest.json"
        hidden_manifest = manifest.with_name("manifest.json.benchmark-disabled")
        if not manifest.is_file():
            raise RuntimeError("The synthetic vault did not receive its model pack.")
        manifest.rename(hidden_manifest)
        try:
            lexical_recalls, lexical_packets = recall_samples(10, "lexical")
        finally:
            hidden_manifest.rename(manifest)

        report = {
            "fixture_atoms": ATOM_COUNT,
            "incremental_atoms": INCREMENTAL_BATCHES * BATCH_SIZE,
            "fixture_seed": FIXTURE_SEED,
            "fixture_observed_at": now,
            "deterministic_atom_ids": deterministic_atom_ids,
            "synthetic_categories": len(CATEGORIES),
            "target_category_atoms": ATOM_COUNT // len(CATEGORIES),
            "indexed_atoms": ATOM_COUNT - int(indexed["pending_atoms"]),
            "all_atoms_confirmed_indexed": indexed["pending_atoms"] == 0,
            "index_refresh_calls": refresh_calls,
            "ingest_ms": round(ingest_ms, 2),
            "full_index_ms": round(indexing_ms, 2),
            "backfill_calls": refresh_samples,
            "incremental_batches": incremental,
            "storage_after_ingest": after_ingest_storage,
            "storage_after_backfill": after_backfill_storage,
            "storage_final": storage_snapshot(),
            "includes_process_launch": True,
            "recall_budget_ms": request["budget_ms"],
            "recall_samples": stats(recalls),
            "lexical_recall_samples": stats(lexical_recalls),
            "zero_pending_refresh_samples": stats(no_op_refresh),
            "raw_recall_ms": recalls,
            "raw_lexical_recall_ms": lexical_recalls,
            "raw_zero_pending_refresh_ms": no_op_refresh,
            "recall_statuses": sorted(
                {packet.get("status", "missing") for packet in final_packets}
            ),
            "context_records_median": round(
                statistics.median(
                    len(packet.get("context", [])) for packet in final_packets
                ),
                1,
            ),
            "lexical_context_records_median": round(
                statistics.median(
                    len(packet.get("context", [])) for packet in lexical_packets
                ),
                1,
            ),
            "elapsed_total_ms": round((time.perf_counter() - started) * 1000, 2),
            "platform": platform.system(),
            "architecture": platform.machine(),
            "cache_state": "warm after full index and one discarded recall call",
            "fixture_scope": "generated metadata only; isolated temporary Serein directory",
            "reference_machine": "Apple silicon macOS host; not the specified dual-core/4 GiB laptop",
            "peak_rss_bytes_child_processes_during_harness": int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss * (1 if sys.platform == "darwin" else 1024)) if resource else None,
            "rss_scope": "Maximum of sequential helper/CLI child processes, including ingest and backfill; not recall alone.",
            "maintenance_work_scope": "Processed counts and persisted row counts are measured here; internal row reads and encoder calls require the targeted source tests.",
            "storage_scope": "Database/WAL/SHM snapshots after child process exit; transient peak WAL bytes are not measured.",
        }
        if output := os.environ.get("SEREIN_BENCH_OUTPUT"):
            pathlib.Path(output).write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
