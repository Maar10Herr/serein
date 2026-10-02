#!/usr/bin/env python3
"""Run paired synthetic checks, with a hash-guarded continuation for untouched calls."""

from __future__ import annotations

import argparse
import collections
import copy
import datetime
from contextlib import closing
import hashlib
import json
import os
import pathlib
import re
import sqlite3
import struct
import subprocess
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
SLICES = (
    "ordinary_positive",
    "multilingual",
    "comparison_direction",
    "correction_state",
    "duplicate_contamination",
    "expected_empty",
)
MAX_CONTEXT_RECORDS = 6
MAX_PACKET_BYTES = 4096
REFRESH_CALL_LIMIT = 100
REFRESH_BUDGET_MS = 5000
RECALL_BUDGET_MS = 1500


def deterministic_id(seed: str, episode_id: str, label: str) -> str:
    namespace = uuid.uuid5(
        uuid.NAMESPACE_URL, f"serein-fresh-validation:{seed}:{episode_id}"
    )
    return str(uuid.uuid5(namespace, label))


def run_json(
    command: list[str], *, env: dict[str, str], request: object | None = None, timeout: int = 15
) -> tuple[dict[str, object], bytes]:
    payload = json.dumps(request, ensure_ascii=False, separators=(",", ":")).encode() if request is not None else None
    result = subprocess.run(
        command,
        input=payload,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
        timeout=timeout,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"command exited {result.returncode}: {command!r}; "
            f"stdout={result.stdout.decode(errors='replace')!r}; "
            f"stderr={result.stderr.decode(errors='replace')!r}"
        )
    try:
        return json.loads(result.stdout), result.stdout
    except json.JSONDecodeError as error:
        raise RuntimeError(
            f"command returned invalid JSON: {command!r}; "
            f"stdout={result.stdout.decode(errors='replace')!r}"
        ) from error


def validate_fixture(fixture: object) -> dict[str, object]:
    if not isinstance(fixture, dict):
        raise ValueError("Fixture root must be a JSON object.")
    fixture_id = fixture.get("fixture_id")
    seed = fixture.get("seed")
    episodes = fixture.get("episodes")
    if not isinstance(fixture_id, str) or not fixture_id:
        raise ValueError("Fixture requires a nonempty fixture_id.")
    if isinstance(seed, str):
        if not seed:
            raise ValueError("Fixture requires a nonempty seed.")
    elif type(seed) is int:
        fixture["seed"] = seed = str(seed)
    else:
        raise ValueError("Fixture seed must be a nonempty string or integer.")
    if not isinstance(episodes, list) or len(episodes) != 24:
        raise ValueError("Fixture must contain exactly 24 episodes.")

    counts = collections.Counter()
    episode_ids = set()
    for episode in episodes:
        if not isinstance(episode, dict):
            raise ValueError("Every episode must be an object.")
        episode_id = episode.get("id")
        slice_name = episode.get("slice")
        if not isinstance(episode_id, str) or not episode_id or episode_id in episode_ids:
            raise ValueError("Episode IDs must be nonempty and unique.")
        if slice_name not in SLICES:
            raise ValueError(f"Unknown episode slice: {slice_name!r}.")
        episode_ids.add(episode_id)
        counts[slice_name] += 1

        observations = episode.get("observations")
        if not isinstance(observations, list):
            raise ValueError(f"Episode {episode_id} requires an observations list.")
        aliases = set()
        for observation in observations:
            if not isinstance(observation, dict):
                raise ValueError(f"Episode {episode_id} contains a non-object observation.")
            alias = observation.get("id")
            title = observation.get("title")
            host = observation.get("host")
            query = observation.get("query")
            kind = observation.get("kind")
            dwell = observation.get("dwell_seconds")
            if not isinstance(alias, str) or not alias or alias in aliases:
                raise ValueError(f"Episode {episode_id} observation aliases must be unique strings.")
            if not isinstance(title, str) or len(title) > 256:
                raise ValueError(f"Episode {episode_id} has an invalid observation title.")
            if not isinstance(host, str) or not _valid_fixture_host(host):
                raise ValueError(f"Episode {episode_id} has an invalid observation host.")
            if query is not None and not isinstance(query, str):
                raise ValueError(f"Episode {episode_id} has an invalid observation query.")
            if isinstance(query, str) and len(query) > 512:
                raise ValueError(f"Episode {episode_id} observation query is too long.")
            if not isinstance(kind, str) or kind not in {"visit", "search"}:
                raise ValueError(f"Episode {episode_id} has an invalid observation kind.")
            if type(dwell) is not int or not 0 <= dwell <= 3600:
                raise ValueError(f"Episode {episode_id} has invalid dwell_seconds.")
            if len(title.encode()) + (len(query.encode()) if query is not None else 0) > 2048:
                raise ValueError(f"Episode {episode_id} observation text exceeds protocol bounds.")
            aliases.add(alias)

        question = episode.get("question")
        facets = episode.get("facets", [])
        if not isinstance(question, str) or len(question) > 512:
            raise ValueError(f"Episode {episode_id} requires a bounded question.")
        if not isinstance(facets, list) or len(facets) > 3 or any(
            not isinstance(facet, str) or len(facet) > 128 for facet in facets
        ):
            raise ValueError(f"Episode {episode_id} has invalid recall facets.")
        if not isinstance(episode.get("feedback", []), list):
            raise ValueError(f"Episode {episode_id} feedback must be a list.")
        for feedback in episode.get("feedback", []):
            if (
                not isinstance(feedback, dict)
                or not isinstance(feedback.get("observation_id"), str)
                or feedback.get("observation_id") not in aliases
            ):
                raise ValueError(f"Episode {episode_id} feedback references an unknown observation.")
            action = feedback.get("action")
            if not isinstance(action, str) or action not in {
                "not_about_me",
                "wrong_topic",
                "temporary_research",
                "confirm_constraint",
                "do_not_use",
            }:
                raise ValueError(f"Episode {episode_id} feedback requires an action.")
            if feedback.get("text") is not None and not isinstance(feedback.get("text"), str):
                raise ValueError(f"Episode {episode_id} feedback text must be a string or null.")

        for field in ("useful_ids", "forbidden_ids"):
            values = episode.get(field)
            if not isinstance(values, list) or any(
                not isinstance(value, str) or value not in aliases for value in values
            ):
                raise ValueError(f"Episode {episode_id} {field} must reference observation aliases.")
        if not isinstance(episode.get("expected_empty"), bool):
            raise ValueError(f"Episode {episode_id} expected_empty must be boolean.")
        expected_states = episode.get("expected_states", {})
        if not isinstance(expected_states, dict) or any(
            alias not in aliases or not isinstance(state, str)
            for alias, state in expected_states.items()
        ):
            raise ValueError(f"Episode {episode_id} has invalid expected_states.")

        groups = episode.get("useful_groups", [])
        minimum_groups = episode.get("minimum_useful_groups", 0)
        if not isinstance(groups, list) or any(
            not isinstance(group, list)
            or not group
            or any(not isinstance(alias, str) or alias not in aliases for alias in group)
            for group in groups
        ):
            raise ValueError(f"Episode {episode_id} has invalid useful_groups.")
        if type(minimum_groups) is not int or not 0 <= minimum_groups <= len(groups):
            raise ValueError(f"Episode {episode_id} has invalid minimum_useful_groups.")
        if len({frozenset(group) for group in groups}) != len(groups):
            raise ValueError(f"Episode {episode_id} repeats a useful group.")

    if set(counts) != set(SLICES) or any(counts[name] != 4 for name in SLICES):
        raise ValueError(f"Fixture must have exactly four episodes in every slice: {dict(counts)}")
    return fixture


def _valid_fixture_host(host: str) -> bool:
    if not host or len(host) > 253 or host != host.lower() or "." not in host:
        return False
    if not host.isascii() or any(
        not label
        or label.startswith("-")
        or label.endswith("-")
        or not re.fullmatch(r"[a-z0-9-]+", label)
        for label in host.split(".")
    ):
        return False
    return True


def readonly_event_atoms(
    database_path: pathlib.Path, source: str, event_ids: dict[str, str]
) -> dict[str, str]:
    """Resolve event IDs to atoms using a read-only SQLite connection."""
    alias_to_atom = {}
    uri = database_path.as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True, timeout=5)) as database:
        database.execute("PRAGMA query_only=ON")
        for alias, event_id in event_ids.items():
            row = database.execute(
                "SELECT atom FROM events WHERE source=? AND id=?", (source, event_id)
            ).fetchone()
            if row is None:
                raise RuntimeError(f"Ingested event {alias!r} is missing from the fixture vault.")
            alias_to_atom[alias] = row[0]
    return alias_to_atom


def normalize_fixture_atom_ids(
    database_path: pathlib.Path,
    *,
    seed: str,
    episode_id: str,
    alias_to_old_atom: dict[str, str],
) -> dict[str, str]:
    """Replace random disposable-vault atom IDs with stable UUIDv5 IDs."""
    aliases_by_old_atom: dict[str, list[str]] = collections.defaultdict(list)
    for alias, atom_id in alias_to_old_atom.items():
        aliases_by_old_atom[atom_id].append(alias)
    old_to_new = {
        old_id: deterministic_id(seed, episode_id, f"atom:{min(aliases)}")
        for old_id, aliases in aliases_by_old_atom.items()
    }
    if len(set(old_to_new.values())) != len(old_to_new):
        raise RuntimeError("UUIDv5 atom ID collision in the synthetic fixture.")
    alias_to_new = {
        alias: old_to_new[old_id] for alias, old_id in alias_to_old_atom.items()
    }
    if not old_to_new:
        return alias_to_new

    database = sqlite3.connect(database_path, timeout=5)
    try:
        database.execute("PRAGMA foreign_keys=OFF")
        if database.execute("PRAGMA foreign_keys").fetchone()[0] != 0:
            raise RuntimeError("Could not disable SQLite foreign keys for test-only ID normalization.")
        tables = {
            row[0]
            for row in database.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            )
        }
        for table in (
            "vectors",
            "topics",
            "atom_topics",
            "topic_skips",
            "feedback",
            "topic_cache",
            "topic_cache_buckets",
        ):
            if table in tables:
                count = database.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                if count != 0:
                    raise RuntimeError(
                        f"Refusing atom ID normalization after {table} was populated."
                    )

        current_ids = {
            row[0] for row in database.execute("SELECT id FROM atoms")
        }
        collisions = {
            new_id
            for old_id, new_id in old_to_new.items()
            if new_id in current_ids and new_id != old_id
        }
        if collisions:
            raise RuntimeError("UUIDv5 atom ID overlaps an unrelated fixture atom.")
        fts_rows = {}
        for old_id in old_to_new:
            rows = database.execute(
                "SELECT id,title,query FROM atom_fts WHERE id=?", (old_id,)
            ).fetchall()
            if len(rows) != 1:
                raise RuntimeError(f"Expected one FTS row for disposable atom {old_id}.")
            fts_rows[old_id] = rows[0]

        database.execute("BEGIN IMMEDIATE")
        for old_id, new_id in old_to_new.items():
            if old_id == new_id:
                continue
            database.execute("UPDATE atoms SET id=? WHERE id=?", (new_id, old_id))
            database.execute("UPDATE events SET atom=? WHERE atom=?", (new_id, old_id))
            database.execute("UPDATE atom_days SET atom=? WHERE atom=?", (new_id, old_id))
            database.execute("DELETE FROM atom_fts WHERE id=?", (old_id,))
            _, title, query = fts_rows[old_id]
            database.execute(
                "INSERT INTO atom_fts(id,title,query) VALUES(?,?,?)",
                (new_id, title, query),
            )
        violations = database.execute("PRAGMA foreign_key_check").fetchall()
        if violations:
            raise RuntimeError(f"Foreign-key check failed after ID normalization: {violations!r}")
        database.commit()
        database.execute("PRAGMA foreign_keys=ON")
        if database.execute("PRAGMA foreign_keys").fetchone()[0] != 1:
            raise RuntimeError("Could not re-enable SQLite foreign keys after normalization.")
        if database.execute("PRAGMA foreign_key_check").fetchone() is not None:
            raise RuntimeError("Foreign-key check failed after re-enabling SQLite constraints.")
    except Exception:
        if database.in_transaction:
            database.rollback()
        raise
    finally:
        database.close()
    return alias_to_new


def prepare_observation_events(
    episode: dict[str, object], *, seed: str, observed_at: str
) -> tuple[list[dict[str, object]], dict[str, str]]:
    events = []
    event_ids = {}
    for observation in episode["observations"]:
        alias = observation["id"]
        event_id = deterministic_id(seed, episode["id"], f"event:{alias}")
        event_ids[alias] = event_id
        events.append(
            {
                "event_id": event_id,
                "visit_id": deterministic_id(seed, episode["id"], f"visit:{alias}"),
                "site_key": observation["host"],
                "site_epoch": 0,
                "observed_at": observed_at,
                "kind": observation["kind"],
                "title": observation["title"],
                "search_query": observation["query"],
                "foreground_seconds": observation["dwell_seconds"],
            }
        )
    return events, event_ids


def invoke_native(
    host: pathlib.Path,
    *,
    env: dict[str, str],
    extension_id: str,
    source: str,
    seed: str,
    episode_id: str,
    call_index: int,
    op: str,
    payload: dict[str, object],
) -> dict[str, object]:
    request_id = deterministic_id(seed, episode_id, f"native:{call_index}:{op}")
    request = {
        "protocol": 1,
        "request_id": request_id,
        "source_id": source,
        "op": op,
        "capture_epoch": 2,
        "payload": payload,
    }
    body = json.dumps(request, ensure_ascii=False, separators=(",", ":")).encode()
    framed = struct.pack("=I", len(body)) + body
    result = subprocess.run(
        [str(host), f"chrome-extension://{extension_id}/"],
        input=framed,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
        timeout=10,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"native {op} exited {result.returncode}: "
            f"stdout={result.stdout.decode(errors='replace')!r}; "
            f"stderr={result.stderr.decode(errors='replace')!r}"
        )
    if len(result.stdout) < 4:
        raise RuntimeError(f"Native {op} returned an incomplete frame.")
    size = struct.unpack("=I", result.stdout[:4])[0]
    if size > 256 * 1024 or size != len(result.stdout) - 4:
        raise RuntimeError(f"Native {op} returned a malformed frame.")
    response = json.loads(result.stdout[4:])
    if response.get("request_id") != request_id or response.get("status") == "error":
        raise RuntimeError(f"Native {op} failed: {response!r}")
    return response


def run_episode(
    binary_name: str,
    cli: pathlib.Path,
    host: pathlib.Path,
    *,
    fixture: dict[str, object],
    episode: dict[str, object],
    extension_id: str,
    observed_at: str,
) -> dict[str, object]:
    seed = fixture["seed"]
    episode_id = episode["id"]
    outcome: dict[str, object] = {
        "binary": binary_name,
        "episode_id": episode_id,
        "slice": episode["slice"],
        "execution_status": "error",
        "recall_attempted": False,
        "recall_call_complete": False,
        "assessment_status": "not_run",
        "assessment_error": None,
        "error_stage": None,
        "error": None,
        "raw_packet": None,
        "packet": None,
        "ingest_batches": [],
        "accepted_observations": [],
        "rejected_observations": [],
        "feedback_applied": [],
        "feedback_skipped_native_rejected": [],
    }
    stage = "temporary-vault"
    with tempfile.TemporaryDirectory(prefix=f"serein-fresh-{binary_name}-") as temp_dir:
        temp = pathlib.Path(temp_dir).resolve()
        data_dir = temp / "data"
        install_home = temp / "install home"
        scratch = temp / "temporary files"
        install_home.mkdir()
        scratch.mkdir()
        env = {
            **os.environ,
            "SEREIN_DATA_DIR": str(data_dir),
            "SEREIN_INSTALL_HOME": str(install_home),
            "TMPDIR": str(scratch),
        }
        try:
            source = deterministic_id(seed, episode_id, "source")
            nonce = deterministic_id(seed, episode_id, "nonce-a") + deterministic_id(
                seed, episode_id, "nonce-b"
            )
            setup = {
                "protocol": 1,
                "source_id": source,
                "extension_id": extension_id,
                "browser": "chrome",
                "nonce": nonce,
                "expires_at": int(time.time()) + 890,
                "adapters": ["generic"],
                "label": "Fresh synthetic validation fixture",
                "consent": True,
            }
            stage = "setup"
            connection, _ = run_json(
                [str(cli), "setup", "--request-stdin", "--json"],
                env=env,
                request=setup,
            )
            if connection.get("status") != "ok":
                raise RuntimeError(f"Synthetic setup failed: {connection!r}")
            database_path = pathlib.Path(connection["database_path"])
            if not database_path.is_file() or data_dir not in database_path.parents:
                raise RuntimeError("Setup did not create the disposable fixture vault under its data directory.")

            call_index = 0

            def native(op: str, payload: dict[str, object]) -> dict[str, object]:
                nonlocal call_index
                response = invoke_native(
                    host,
                    env=env,
                    extension_id=extension_id,
                    source=source,
                    seed=seed,
                    episode_id=episode_id,
                    call_index=call_index,
                    op=op,
                    payload=payload,
                )
                call_index += 1
                return response

            stage = "hello"
            hello = native("hello", {"nonce": nonce})
            if hello.get("status") != "ok":
                raise RuntimeError(f"Native pairing failed: {hello!r}")
            stage = "policy"
            policy = native(
                "policy.update",
                {
                    "consent": True,
                    "paused": False,
                    "recall_enabled": True,
                    "selected_only": False,
                    "selected_sites": [],
                    "excluded_sites": [],
                    "capture_epoch": 2,
                },
            )
            if not policy.get("policy", {}).get("recall_enabled"):
                raise RuntimeError(f"Native policy update was not applied: {policy!r}")

            events, event_ids = prepare_observation_events(
                episode, seed=seed, observed_at=observed_at
            )
            stage = "ingest"
            accepted_aliases = set()
            rejected_by_alias: dict[str, str] = {}
            event_aliases = {event_id: alias for alias, event_id in event_ids.items()}
            for offset in range(0, len(events), 32):
                batch = events[offset : offset + 32]
                ingest = native("ingest", {"events": batch})
                expected_ids = [event["event_id"] for event in batch]
                acknowledged = ingest.get("acknowledged_ids")
                rejected = ingest.get("rejected")
                if (
                    ingest.get("status") != "ok"
                    or not isinstance(acknowledged, list)
                    or any(not isinstance(event_id, str) for event_id in acknowledged)
                    or len(acknowledged) != len(set(acknowledged))
                    or not isinstance(rejected, list)
                ):
                    raise RuntimeError("Native ingest returned a malformed accepted/rejected partition.")
                rejected_by_event: dict[str, str] = {}
                for item in rejected:
                    if (
                        not isinstance(item, dict)
                        or not isinstance(item.get("id"), str)
                        or not isinstance(item.get("reason"), str)
                        or not item["reason"]
                        or item["id"] in rejected_by_event
                    ):
                        raise RuntimeError("Native ingest returned a malformed rejection entry.")
                    rejected_by_event[item["id"]] = item["reason"]
                accepted_ids = set(acknowledged)
                rejected_ids = set(rejected_by_event)
                expected_set = set(expected_ids)
                if (
                    len(rejected_by_event) != len(rejected)
                    or accepted_ids & rejected_ids
                    or accepted_ids | rejected_ids != expected_set
                    or len(acknowledged) + len(rejected) != len(expected_ids)
                ):
                    raise RuntimeError("Native ingest did not partition the complete fixture batch.")

                accepted_batch_aliases = []
                rejected_batch_aliases = []
                for event_id in expected_ids:
                    alias = event_aliases[event_id]
                    if event_id in accepted_ids:
                        accepted_aliases.add(alias)
                        accepted_batch_aliases.append(alias)
                    else:
                        reason = rejected_by_event[event_id]
                        rejected_by_alias[alias] = reason
                        rejected_observation = {
                            "alias": alias,
                            "event_id": event_id,
                            "reason": reason,
                        }
                        outcome["rejected_observations"].append(rejected_observation)
                        rejected_batch_aliases.append(rejected_observation)
                outcome["ingest_batches"].append(
                    {
                        "accepted_aliases": accepted_batch_aliases,
                        "rejected_observations": rejected_batch_aliases,
                        "partition_complete": True,
                    }
                )

            accepted_event_ids = {
                alias: event_id
                for alias, event_id in event_ids.items()
                if alias in accepted_aliases
            }
            outcome["accepted_observations"] = [
                {"alias": alias, "event_id": event_id}
                for alias, event_id in accepted_event_ids.items()
            ]
            outcome["observation_event_ids"] = accepted_event_ids

            stage = "event-id-map"
            old_alias_to_atom = readonly_event_atoms(database_path, source, accepted_event_ids)
            if set(old_alias_to_atom) != set(accepted_event_ids):
                raise RuntimeError("Read-only event mapping did not cover every accepted fixture observation.")
            stage = "normalize-ids"
            alias_to_atom = normalize_fixture_atom_ids(
                database_path,
                seed=seed,
                episode_id=episode_id,
                alias_to_old_atom=old_alias_to_atom,
            )
            verified_alias_to_atom = readonly_event_atoms(database_path, source, accepted_event_ids)
            if verified_alias_to_atom != alias_to_atom:
                raise RuntimeError("Deterministic atom ID normalization did not persist exactly.")
            atom_to_aliases: dict[str, list[str]] = collections.defaultdict(list)
            for alias, atom_id in alias_to_atom.items():
                atom_to_aliases[atom_id].append(alias)
            atom_to_aliases = {
                atom_id: sorted(aliases) for atom_id, aliases in atom_to_aliases.items()
            }

            stage = "feedback"
            for feedback_index, feedback in enumerate(episode.get("feedback", [])):
                alias = feedback["observation_id"]
                if alias in rejected_by_alias:
                    outcome["feedback_skipped_native_rejected"].append(
                        {
                            "observation_id": alias,
                            "action": feedback["action"],
                            "rejection_reason": rejected_by_alias[alias],
                        }
                    )
                    continue
                if alias not in alias_to_atom:
                    raise RuntimeError(
                        f"Feedback {feedback_index} references an alias that was neither accepted nor rejected."
                    )
                response = native(
                    "feedback",
                    {
                        "atom_id": alias_to_atom[alias],
                        "action": feedback["action"],
                        "text": feedback.get("text"),
                    },
                )
                if response.get("status") != "ok":
                    raise RuntimeError(f"Feedback {feedback_index} was rejected: {response!r}")
                outcome["feedback_applied"].append(
                    {"observation_id": alias, "action": feedback["action"]}
                )

            stage = "refresh"
            refresh = None
            for _ in range(REFRESH_CALL_LIMIT):
                refresh, _ = run_json(
                    [
                        str(cli),
                        "refresh",
                        "--budget-ms",
                        str(REFRESH_BUDGET_MS),
                        "--json",
                    ],
                    env=env,
                    timeout=15,
                )
                if refresh.get("mode") != "hybrid":
                    raise RuntimeError(f"Refresh did not use the bundled model: {refresh!r}")
                if refresh.get("pending_atoms") == 0:
                    break
            else:
                raise RuntimeError(f"Refresh did not reach zero pending atoms: {refresh!r}")
            if refresh is None or refresh.get("pending_atoms") != 0:
                raise RuntimeError(f"Refresh was incomplete: {refresh!r}")

            request_id = deterministic_id(seed, episode_id, "recall")
            request = {
                "protocol": 1,
                "request_id": request_id,
                "client": "generic",
                "vault": "default",
                "query": episode["question"],
                "facets": episode.get("facets", []),
                "scope": ["projects", "research", "confirmed_preferences"],
                "max_bytes": MAX_PACKET_BYTES,
                "budget_ms": RECALL_BUDGET_MS,
            }
            stage = "recall"
            outcome["recall_attempted"] = True
            packet, stdout = run_json(
                [str(cli), "recall", "--request-stdin", "--json"],
                env=env,
                request=request,
                timeout=10,
            )
            outcome["execution_status"] = "complete"
            outcome["recall_call_complete"] = True
            packet_bytes = stdout[:-1] if stdout.endswith(b"\n") else stdout
            trailing_data = len(stdout) - len(packet_bytes) > 1
            outcome["raw_packet"] = packet_bytes.decode("utf-8", errors="replace")
            outcome["packet"] = packet
            if trailing_data:
                outcome["assessment_status"] = "error"
                outcome["assessment_error"] = "Recall output has trailing data beyond its JSON line."
            else:
                outcome["assessment_status"] = "complete"
                try:
                    outcome.update(
                        evaluate_packet(
                            binary_name,
                            episode,
                            packet,
                            packet_bytes,
                            atom_to_aliases,
                            request_id,
                        )
                    )
                except Exception as error:
                    outcome["assessment_status"] = "error"
                    outcome["assessment_error"] = f"{type(error).__name__}: {error}"
            outcome["refresh"] = refresh
            outcome["observation_event_ids"] = accepted_event_ids
            outcome["observation_atom_ids"] = alias_to_atom
            outcome["error_stage"] = None
        except Exception as error:
            if outcome["execution_status"] != "complete":
                outcome["error_stage"] = stage
                outcome["error"] = f"{type(error).__name__}: {error}"
                outcome["execution_status"] = "error"
            else:
                outcome["assessment_status"] = "error"
                outcome["assessment_error"] = f"{type(error).__name__}: {error}"
    return outcome


def evaluate_packet(
    binary_name: str,
    episode: dict[str, object],
    packet: dict[str, object],
    packet_bytes: bytes,
    atom_to_aliases: dict[str, list[str]],
    request_id: str,
) -> dict[str, object]:
    context_value = packet.get("context")
    context_is_list = isinstance(context_value, list)
    context = context_value
    if not context_is_list:
        context = []
    ordered_aliases = []
    details = []
    unmapped_ids = []
    states_by_alias: dict[str, list[str]] = collections.defaultdict(list)
    for record in context:
        if not isinstance(record, dict):
            unmapped_ids.append("<non-object-context-record>")
            ordered_aliases.append([])
            details.append({"record": record, "aliases": []})
            continue
        atom_id = record.get("id")
        aliases = atom_to_aliases.get(atom_id, []) if isinstance(atom_id, str) else []
        if not aliases:
            unmapped_ids.append(atom_id if isinstance(atom_id, str) else "<missing-id>")
        ordered_aliases.append(aliases)
        state = record.get("state")
        for alias in aliases:
            if isinstance(state, str):
                states_by_alias[alias].append(state)
        details.append(
            {
                "atom_id": atom_id,
                "aliases": aliases,
                "text": record.get("text"),
                "state": state,
                "record": record,
            }
        )

    present_aliases = {
        alias for aliases in ordered_aliases for alias in aliases
    }
    useful_ids = set(episode.get("useful_ids", []))
    forbidden_ids = set(episode.get("forbidden_ids", []))
    expected_states = episode.get("expected_states", {})
    misses = sorted(useful_ids - present_aliases)
    forbidden_included = sorted(forbidden_ids & present_aliases)
    state_mismatches = []
    for alias, expected_state in expected_states.items():
        actual = states_by_alias.get(alias, [])
        if not actual or any(state != expected_state for state in actual):
            state_mismatches.append(
                {"alias": alias, "expected": expected_state, "actual": actual}
            )

    groups = episode.get("useful_groups", [])
    group_results = []
    for group in groups:
        group_aliases = set(group)
        matching_record_ids = {
            record.get("id")
            for record, aliases in zip(context, ordered_aliases)
            if isinstance(record, dict)
            and isinstance(record.get("id"), str)
            and group_aliases.intersection(aliases)
        }
        group_results.append(
            {
                "aliases": group,
                "present": sorted(group_aliases & present_aliases),
                "distinct_record_ids": sorted(matching_record_ids),
                "duplicate_slot_failure": len(matching_record_ids) > 1,
            }
        )
    satisfied_groups = sum(bool(group["present"]) for group in group_results)
    duplicate_slot_failures = [
        group for group in group_results if group["duplicate_slot_failure"]
    ]
    minimum_groups = episode.get("minimum_useful_groups", 0)
    unsupported_aliases = sorted(
        present_aliases
        - useful_ids
        - forbidden_ids
        - set(expected_states)
        - {alias for group in groups for alias in group}
    )
    packet_json_size = len(packet_bytes)
    records_well_formed = all(
        isinstance(record, dict)
        and isinstance(record.get("id"), str)
        and isinstance(record.get("text"), str)
        and isinstance(record.get("state"), str)
        and isinstance(record.get("limits"), list)
        for record in context
    )
    raw_atom_ids = [
        record.get("id") for record in context if isinstance(record, dict)
    ]
    raw_id_strings = [value for value in raw_atom_ids if isinstance(value, str)]
    raw_id_uniqueness_ok = len(raw_id_strings) == len(raw_atom_ids) and len(
        raw_id_strings
    ) == len(set(raw_id_strings))
    packet_index = packet.get("index")
    if not isinstance(packet_index, dict):
        packet_index = {}
    hybrid_ok = packet_index.get("mode") == "hybrid" and packet_index.get("pending_atoms") == 0
    expected_empty = episode["expected_empty"]
    observed_empty = len(context) == 0
    request_id_ok = packet.get("request_id") == request_id
    protocol_ok = packet.get("protocol") == 1
    hardbounds_ok = (
        packet_json_size <= MAX_PACKET_BYTES
        and len(context) <= MAX_CONTEXT_RECORDS
        and context_is_list
        and records_well_formed
        and raw_id_uniqueness_ok
        and request_id_ok
        and protocol_ok
    )
    privacy_ok = not unmapped_ids and not forbidden_included
    useful_groups_ok = satisfied_groups >= minimum_groups
    duplicate_slots_ok = not duplicate_slot_failures
    expected_empty_ok = observed_empty == expected_empty
    checks = {
        "hardbounds_ok": hardbounds_ok,
        "raw_id_uniqueness_ok": raw_id_uniqueness_ok,
        "duplicate_slots_ok": duplicate_slots_ok,
        "duplicate_slot_failures": duplicate_slot_failures,
        "hybrid_index_ok": hybrid_ok,
        "index_mode": packet_index.get("mode"),
        "pending_atoms": packet_index.get("pending_atoms"),
        "packet_bytes": packet_json_size,
        "max_packet_bytes": MAX_PACKET_BYTES,
        "context_records": len(context),
        "max_context_records": MAX_CONTEXT_RECORDS,
        "records_well_formed": records_well_formed,
        "request_id_ok": request_id_ok,
        "protocol_ok": protocol_ok,
        "privacy_ok": privacy_ok,
        "unmapped_ids": unmapped_ids,
        "forbidden_included": forbidden_included,
        "expected_empty_ok": expected_empty_ok,
        "expected_empty": expected_empty,
        "observed_empty": observed_empty,
        "useful_ids_ok": not misses,
        "misses": misses,
        "useful_groups_ok": useful_groups_ok,
        "minimum_useful_groups": minimum_groups,
        "satisfied_useful_groups": satisfied_groups,
        "state_expectations_ok": not state_mismatches,
        "state_mismatches": state_mismatches,
    }
    checks["passed"] = all(
        checks[key]
        for key in (
            "hardbounds_ok",
            "duplicate_slots_ok",
            "hybrid_index_ok",
            "privacy_ok",
            "expected_empty_ok",
            "useful_ids_ok",
            "useful_groups_ok",
            "state_expectations_ok",
        )
    )
    return {
        "binary": binary_name,
        "execution_status": "complete",
        "raw_packet": packet_bytes.decode("utf-8"),
        "packet": packet,
        "index": packet.get("index"),
        "status": packet.get("status"),
        "ordered_aliases": ordered_aliases,
        "record_details": details,
        "present_aliases": sorted(present_aliases),
        "raw_atom_ids": raw_atom_ids,
        "unmapped_ids": unmapped_ids,
        "misses": misses,
        "forbidden_included": forbidden_included,
        "new_unsupported_inclusion": unsupported_aliases,
        "useful_groups": group_results,
        "duplicate_slot_failures": duplicate_slot_failures,
        "state_mismatches": state_mismatches,
        "checks": checks,
        "_states_by_alias": {alias: states for alias, states in states_by_alias.items()},
    }


def compare_episode_runs(
    baseline: dict[str, object], candidate: dict[str, object]
) -> dict[str, object]:
    if (
        baseline.get("execution_status") != "complete"
        or candidate.get("execution_status") != "complete"
        or baseline.get("assessment_status") != "complete"
        or candidate.get("assessment_status") != "complete"
    ):
        return {
            "changed_ids": None,
            "text_changed_aliases": None,
            "state_changed_aliases": None,
            "new_unsupported_inclusion": None,
            "comparison_status": "incomplete",
        }
    baseline_aliases = set(baseline["present_aliases"])
    candidate_aliases = set(candidate["present_aliases"])
    baseline_unsupported = set(baseline["new_unsupported_inclusion"])
    candidate_unsupported = set(candidate["new_unsupported_inclusion"])
    baseline_state = baseline["_states_by_alias"]
    candidate_state = candidate["_states_by_alias"]

    def alias_texts(run: dict[str, object]) -> dict[str, set[str]]:
        values: dict[str, set[str]] = collections.defaultdict(set)
        for record in run["record_details"]:
            text = record.get("text")
            if isinstance(text, str):
                for alias in record["aliases"]:
                    values[alias].add(text)
        return values

    base_text = alias_texts(baseline)
    cand_text = alias_texts(candidate)
    common = baseline_aliases & candidate_aliases
    text_changed = sorted(alias for alias in common if base_text.get(alias) != cand_text.get(alias))
    state_changed = sorted(
        alias for alias in common if baseline_state.get(alias) != candidate_state.get(alias)
    )
    return {
        "comparison_status": "complete",
        "changed_ids": {
            "baseline_only_aliases": sorted(baseline_aliases - candidate_aliases),
            "candidate_only_aliases": sorted(candidate_aliases - baseline_aliases),
            "changed_raw_atom_id_aliases": sorted(
                alias
                for alias in common
                if any(
                    detail.get("atom_id") != other.get("atom_id")
                    for detail in baseline["record_details"]
                    if alias in detail.get("aliases", [])
                    for other in candidate["record_details"]
                    if alias in other.get("aliases", [])
                )
            ),
        },
        "text_changed_aliases": text_changed,
        "state_changed_aliases": state_changed,
        "new_unsupported_inclusion": sorted(candidate_unsupported - baseline_unsupported),
    }


def quality_issue_snapshot(run: dict[str, object]) -> dict[str, object]:
    """Copy per-run quality observations without making them execution failures."""
    checks = run.get("checks")
    if not isinstance(checks, dict):
        checks = None
    false_checks = (
        sorted(
            key
            for key, value in checks.items()
            if key.endswith("_ok") and value is False
        )
        if checks is not None
        else []
    )
    expected_empty = None
    if checks is not None:
        expected_empty = {
            "expected_empty": checks.get("expected_empty"),
            "observed_empty": checks.get("observed_empty"),
            "expected_empty_ok": checks.get("expected_empty_ok"),
        }
    return {
        "assessment_status": run.get("assessment_status"),
        "assessment_error": run.get("assessment_error"),
        "false_checks": false_checks,
        "misses": run.get("misses"),
        "state_mismatches": run.get("state_mismatches"),
        "unsupported_inclusion": run.get("new_unsupported_inclusion"),
        "forbidden_included": run.get("forbidden_included"),
        "expected_empty": expected_empty,
        "duplicate_slot_failures": run.get("duplicate_slot_failures"),
        "useful_groups": checks.get("useful_groups_ok") if checks is not None else None,
        "checks": checks,
    }


def hard_violations(run: dict[str, object]) -> list[dict[str, object]]:
    checks = run.get("checks")
    if not isinstance(checks, dict):
        return []
    violations = []
    if checks.get("hardbounds_ok") is False:
        failed_conditions = []
        if checks.get("packet_bytes", 0) > checks.get("max_packet_bytes", 0):
            failed_conditions.append("packet_byte_limit")
        if checks.get("context_records", 0) > checks.get("max_context_records", 0):
            failed_conditions.append("context_record_limit")
        if checks.get("records_well_formed") is False:
            failed_conditions.append("record_shape")
        if checks.get("raw_id_uniqueness_ok") is False:
            failed_conditions.append("raw_id_uniqueness")
        if checks.get("request_id_ok") is False:
            failed_conditions.append("request_id")
        if checks.get("protocol_ok") is False:
            failed_conditions.append("protocol")
        violations.append(
            {"check": "hardbounds_ok", "failed_conditions": failed_conditions}
        )
    if checks.get("privacy_ok") is False:
        violations.append(
            {
                "check": "privacy_ok",
                "unmapped_ids": checks.get("unmapped_ids"),
                "forbidden_included": checks.get("forbidden_included"),
            }
        )
    if checks.get("expected_empty") is True and checks.get("observed_empty") is False:
        violations.append(
            {
                "check": "expected_empty_ok",
                "expected_empty": checks.get("expected_empty"),
                "observed_empty": checks.get("observed_empty"),
            }
        )
    if checks.get("duplicate_slots_ok") is False:
        violations.append(
            {
                "check": "duplicate_slots_ok",
                "duplicate_slot_failures": checks.get("duplicate_slot_failures"),
            }
        )
    if checks.get("useful_groups_ok") is False:
        violations.append(
            {
                "check": "useful_groups_ok",
                "minimum_useful_groups": checks.get("minimum_useful_groups"),
                "satisfied_useful_groups": checks.get("satisfied_useful_groups"),
                "unsatisfied_groups": [
                    group
                    for group in run.get("useful_groups", [])
                    if not group.get("present")
                ],
            }
        )
    return violations


def compare_hard_violations(
    baseline: dict[str, object], candidate: dict[str, object]
) -> dict[str, object]:
    baseline_violations = hard_violations(baseline)
    candidate_violations = hard_violations(candidate)

    def fingerprint(violation: dict[str, object]) -> str:
        return json.dumps(violation, ensure_ascii=False, sort_keys=True, separators=(",", ":"))

    baseline_by_key = {fingerprint(item): item for item in baseline_violations}
    candidate_by_key = {fingerprint(item): item for item in candidate_violations}
    baseline_keys = set(baseline_by_key)
    candidate_keys = set(candidate_by_key)
    return {
        "baseline": baseline_violations,
        "candidate": candidate_violations,
        "new_candidate_violations": [candidate_by_key[key] for key in sorted(candidate_keys - baseline_keys)],
        "present_in_both": [candidate_by_key[key] for key in sorted(candidate_keys & baseline_keys)],
        "baseline_only_violations": [baseline_by_key[key] for key in sorted(baseline_keys - candidate_keys)],
    }


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_resume_state(
    report_path: pathlib.Path,
    identity_path: pathlib.Path,
    *,
    fixture: dict[str, object],
    fixture_sha256: str,
    binary_paths: dict[str, pathlib.Path],
    binary_sha256: dict[str, str],
) -> tuple[dict[str, object], dict[str, dict[str, object]], dict[str, object]]:
    report_bytes = report_path.read_bytes()
    report_sha256 = hashlib.sha256(report_bytes).hexdigest()
    try:
        prior = json.loads(report_bytes)
        identity = json.loads(identity_path.read_text())
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError(f"Resume report or identity sidecar is invalid JSON: {error}") from error
    if not isinstance(prior, dict) or not isinstance(identity, dict):
        raise ValueError("Resume report and identity sidecar must be JSON objects.")
    if identity.get("original_report_sha256") != report_sha256:
        raise ValueError("Resume report SHA-256 does not match the reviewed identity sidecar.")
    if identity.get("fixture_sha256") != fixture_sha256:
        raise ValueError("Fixture SHA-256 does not match the reviewed identity sidecar.")
    if identity.get("binary_sha256") != binary_sha256:
        raise ValueError("Current CLI/host hashes do not match the reviewed first-run identity.")
    if prior.get("fixture_id") != fixture.get("fixture_id"):
        raise ValueError("Resume report belongs to a different fixture.")
    if prior.get("seed") != fixture.get("seed"):
        raise ValueError("Resume report seed does not match the fixture.")
    if (
        prior.get("fixture_sha256_before") != fixture_sha256
        or prior.get("fixture_sha256_after") != fixture_sha256
        or prior.get("fixture_unchanged") is not True
    ):
        raise ValueError("Resume report does not confirm unchanged fixture bytes.")
    for name, path in binary_paths.items():
        if prior.get(name) != str(path):
            raise ValueError(f"Resume report uses a different {name.replace('_', '-')} path.")

    prior_episodes = prior.get("episodes")
    if not isinstance(prior_episodes, list) or len(prior_episodes) != 24:
        raise ValueError("Resume report must contain all 24 first-run episodes.")
    if [item.get("episode_id") for item in prior_episodes if isinstance(item, dict)] != [
        episode["id"] for episode in fixture["episodes"]
    ]:
        raise ValueError("Resume episode order or IDs do not match the sealed fixture.")

    prior_by_id = {item["episode_id"]: item for item in prior_episodes}
    attempted_count = 0
    completed_count = 0
    unattempted = []
    for episode in fixture["episodes"]:
        saved = prior_by_id[episode["id"]]
        if saved.get("slice") != episode["slice"] or saved.get("question") != episode["question"]:
            raise ValueError(f"Resume episode metadata changed for {episode['id']}.")
        observed_at = saved.get("observed_at")
        if not isinstance(observed_at, str) or not observed_at:
            raise ValueError(f"Resume episode has no original timestamp: {episode['id']}.")
        for side in ("baseline", "candidate"):
            run = saved.get(side)
            if not isinstance(run, dict):
                raise ValueError(f"Resume report lacks {side} outcome for {episode['id']}.")
            attempted = run.get("recall_attempted")
            complete = run.get("recall_call_complete")
            if type(attempted) is not bool or type(complete) is not bool:
                raise ValueError("Resume report lacks recall-attempt provenance.")
            if attempted and not complete:
                raise ValueError(
                    f"Refusing to retry attempted but incomplete recall: {episode['id']} {side}."
                )
            if complete and not attempted:
                raise ValueError("Resume report marks a recall complete without an attempt.")
            if complete:
                if run.get("execution_status") != "complete" or not isinstance(
                    run.get("raw_packet"), str
                ):
                    raise ValueError("A completed recall lacks its original raw response.")
                attempted_count += 1
                completed_count += 1
            else:
                if run.get("execution_status") != "error" or run.get("error_stage") != "ingest":
                    raise ValueError(
                        f"Unattempted recall is not the preserved ingest stop: {episode['id']} {side}."
                    )
                unattempted.append((episode["id"], side))

    previous_execution = prior.get("execution")
    if not isinstance(previous_execution, dict):
        raise ValueError("Resume report has no execution summary.")
    previous_calls = previous_execution.get("recall_calls")
    if (
        not isinstance(previous_calls, dict)
        or previous_calls.get("expected") != 48
        or previous_calls.get("attempted") != attempted_count
        or previous_calls.get("completed") != completed_count
        or attempted_count != 46
        or completed_count != 46
    ):
        raise ValueError("Resume report recall counts do not match its 46 completed calls.")
    if len(unattempted) != 2 or unattempted[0][0] != unattempted[1][0] or {
        side for _, side in unattempted
    } != {"baseline", "candidate"}:
        raise ValueError("Resume is limited to the two untouched sides of one episode.")
    if prior.get("status") != "failed":
        raise ValueError("Resume source must be the preserved incomplete first-run report.")
    return prior, prior_by_id, {
        "prior_report_sha256": report_sha256,
        "identity_sha256": sha256_file(identity_path),
        "prior_recall_calls_completed": completed_count,
        "unattempted_calls": [
            {"episode_id": episode_id, "binary": side}
            for episode_id, side in unattempted
        ],
    }


def run_for_comparison(run: dict[str, object]) -> dict[str, object]:
    """Restore ephemeral comparison state from a completed serialized run."""
    restored = copy.deepcopy(run)
    states_by_alias: dict[str, list[str]] = collections.defaultdict(list)
    for record in run.get("record_details", []):
        state = record.get("state")
        if isinstance(state, str):
            for alias in record.get("aliases", []):
                states_by_alias[alias].append(state)
    restored["_states_by_alias"] = dict(states_by_alias)
    return restored


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=pathlib.Path, required=True)
    parser.add_argument("--baseline-cli", type=pathlib.Path, required=True)
    parser.add_argument("--baseline-host", type=pathlib.Path, required=True)
    parser.add_argument("--candidate-cli", type=pathlib.Path, required=True)
    parser.add_argument("--candidate-host", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument(
        "--resume-from",
        type=pathlib.Path,
        help="preserve completed outcomes and continue only never-attempted recalls",
    )
    parser.add_argument(
        "--resume-identity",
        type=pathlib.Path,
        help="reviewed sidecar pinning the first-report, fixture, and binary hashes",
    )
    args = parser.parse_args()

    fixture_path = args.fixture.resolve(strict=True)
    output_path = args.output.resolve()
    if output_path == fixture_path:
        raise SystemExit("Output path must not overwrite the fixture.")
    for name in ("baseline_cli", "baseline_host", "candidate_cli", "candidate_host"):
        path = getattr(args, name).resolve(strict=True)
        if not path.is_file():
            raise SystemExit(f"{name.replace('_', '-')} is not a file: {path}")
        setattr(args, name, path)
    if (args.resume_from is None) != (args.resume_identity is None):
        raise SystemExit("--resume-from and --resume-identity must be supplied together.")
    if args.resume_from is not None:
        args.resume_from = args.resume_from.resolve(strict=True)
        args.resume_identity = args.resume_identity.resolve(strict=True)
        if output_path in {args.resume_from, args.resume_identity}:
            raise SystemExit("Resume output must preserve both the source report and identity sidecar.")
    if output_path in {
        args.baseline_cli,
        args.baseline_host,
        args.candidate_cli,
        args.candidate_host,
    }:
        raise SystemExit("Output path must not overwrite a CLI or host binary.")
    fixture_bytes = fixture_path.read_bytes()
    fixture_sha_before = hashlib.sha256(fixture_bytes).hexdigest()
    try:
        fixture = validate_fixture(json.loads(fixture_bytes))
    except (json.JSONDecodeError, ValueError) as error:
        raise SystemExit(f"Invalid fresh validation fixture: {error}") from error
    binary_paths = {
        "baseline_cli": args.baseline_cli,
        "baseline_host": args.baseline_host,
        "candidate_cli": args.candidate_cli,
        "candidate_host": args.candidate_host,
    }
    binary_sha256_before = {
        name: sha256_file(path) for name, path in binary_paths.items()
    }
    resume_prior = None
    resume_prior_by_id = None
    resume_metadata = None
    if args.resume_from is not None:
        try:
            resume_prior, resume_prior_by_id, resume_metadata = load_resume_state(
                args.resume_from,
                args.resume_identity,
                fixture=fixture,
                fixture_sha256=fixture_sha_before,
                binary_paths=binary_paths,
                binary_sha256=binary_sha256_before,
            )
        except (ValueError, OSError) as error:
            raise SystemExit(f"Refusing fresh-validation continuation: {error}") from error
    identity_path = ROOT / "release/extension-identity.json"
    extension_id = json.loads(identity_path.read_text())["chrome_extension_id"]
    if not isinstance(extension_id, str) or len(extension_id) != 32:
        raise SystemExit("Release extension identity is invalid.")

    results = []
    continued_calls = []
    if resume_prior_by_id is not None:
        assert resume_metadata is not None
        for episode in fixture["episodes"]:
            saved = copy.deepcopy(resume_prior_by_id[episode["id"]])
            observed_at = saved["observed_at"]
            comparison_runs = {}
            resumed_any = False
            for side, cli, host in (
                ("baseline", args.baseline_cli, args.baseline_host),
                ("candidate", args.candidate_cli, args.candidate_host),
            ):
                previous_run = saved[side]
                if previous_run["recall_call_complete"]:
                    comparison_runs[side] = run_for_comparison(previous_run)
                    continue
                resumed_any = True
                resumed_run = run_episode(
                    side,
                    cli,
                    host,
                    fixture=fixture,
                    episode=episode,
                    extension_id=extension_id,
                    observed_at=observed_at,
                )
                comparison_runs[side] = resumed_run
                saved[side] = {
                    key: value
                    for key, value in resumed_run.items()
                    if not key.startswith("_")
                }
                continued_calls.append(
                    {
                        "episode_id": episode["id"],
                        "binary": side,
                        "observed_at": observed_at,
                        "prior_recall_attempted": False,
                        "prior_error_stage": previous_run.get("error_stage"),
                        "recall_attempted": resumed_run.get("recall_attempted"),
                        "recall_call_complete": resumed_run.get("recall_call_complete"),
                    }
                )
            if resumed_any:
                saved["comparison"] = compare_episode_runs(
                    comparison_runs["baseline"], comparison_runs["candidate"]
                )
            results.append(saved)
    else:
        for episode in fixture["episodes"]:
            observed_at = datetime.datetime.now(datetime.timezone.utc).replace(
                microsecond=0
            ).isoformat().replace("+00:00", "Z")
            baseline = run_episode(
                "baseline",
                args.baseline_cli,
                args.baseline_host,
                fixture=fixture,
                episode=episode,
                extension_id=extension_id,
                observed_at=observed_at,
            )
            candidate = run_episode(
                "candidate",
                args.candidate_cli,
                args.candidate_host,
                fixture=fixture,
                episode=episode,
                extension_id=extension_id,
                observed_at=observed_at,
            )
            results.append(
                {
                    "episode_id": episode["id"],
                    "slice": episode["slice"],
                    "question": episode["question"],
                    "observed_at": observed_at,
                    "expectations": {
                        "useful_ids": episode["useful_ids"],
                        "forbidden_ids": episode["forbidden_ids"],
                        "expected_empty": episode["expected_empty"],
                        "expected_states": episode.get("expected_states", {}),
                        "useful_groups": episode.get("useful_groups", []),
                        "minimum_useful_groups": episode.get("minimum_useful_groups", 0),
                    },
                    "notes": episode.get("notes"),
                    "baseline": {
                        key: value for key, value in baseline.items() if not key.startswith("_")
                    },
                    "candidate": {
                        key: value for key, value in candidate.items() if not key.startswith("_")
                    },
                    "comparison": compare_episode_runs(baseline, candidate),
                }
            )

    fixture_sha_after = sha256_file(fixture_path)
    sha_unchanged = fixture_sha_before == fixture_sha_after
    binary_sha256_after = {
        name: sha256_file(path) for name, path in binary_paths.items()
    }
    binaries_unchanged = binary_sha256_before == binary_sha256_after
    execution_failures = []
    baseline_issues = []
    candidate_issues = []
    baseline_violations = []
    candidate_violations = []
    hard_violation_changes = []
    candidate_new_unsupported = []
    comparison_changes = []
    recall_attempted = 0
    recall_completed = 0
    for result in results:
        for side in ("baseline", "candidate"):
            run = result[side]
            recall_attempted += bool(run.get("recall_attempted"))
            recall_completed += bool(run.get("recall_call_complete"))
            if (
                run.get("execution_status") != "complete"
                or not run.get("recall_call_complete")
            ):
                execution_failures.append(
                    {
                        "episode_id": result["episode_id"],
                        "binary": side,
                        "stage": run.get("error_stage"),
                        "error": run.get("error"),
                    }
                )
            snapshot = quality_issue_snapshot(run)
            has_quality_issue = (
                snapshot["assessment_status"] != "complete"
                or bool(snapshot["false_checks"])
                or bool(snapshot["unsupported_inclusion"])
            )
            if has_quality_issue:
                issue = {"episode_id": result["episode_id"], **snapshot}
                (baseline_issues if side == "baseline" else candidate_issues).append(issue)
            if side == "baseline":
                violations = hard_violations(run)
                if violations:
                    baseline_violations.append(
                        {"episode_id": result["episode_id"], "violations": violations}
                    )
            else:
                violations = hard_violations(run)
                if violations:
                    candidate_violations.append(
                        {"episode_id": result["episode_id"], "violations": violations}
                    )
                new_unsupported = result["comparison"].get("new_unsupported_inclusion")
                candidate_new_unsupported.append(
                    {
                        "episode_id": result["episode_id"],
                        "aliases": new_unsupported,
                    }
                )
        hard_comparison = compare_hard_violations(result["baseline"], result["candidate"])
        if (
            hard_comparison["new_candidate_violations"]
            or hard_comparison["present_in_both"]
            or hard_comparison["baseline_only_violations"]
        ):
            hard_violation_changes.append(
                {"episode_id": result["episode_id"], **hard_comparison}
            )
        comparison = result["comparison"]
        if comparison.get("comparison_status") == "complete":
            changed_ids = comparison.get("changed_ids")
            text_changes = comparison.get("text_changed_aliases")
            state_changes = comparison.get("state_changed_aliases")
            if (
                changed_ids
                and any(changed_ids.values())
                or text_changes
                or state_changes
            ):
                comparison_changes.append(
                    {
                        "episode_id": result["episode_id"],
                        "changed_ids": changed_ids,
                        "text_changed_aliases": text_changes,
                        "state_changed_aliases": state_changes,
                        "new_unsupported_inclusion": comparison.get(
                            "new_unsupported_inclusion"
                        ),
                    }
                )
    if not sha_unchanged:
        execution_failures.append(
            {
                "episode_id": None,
                "binary": None,
                "stage": "fixture-integrity",
                "error": "Fixture SHA-256 changed during the run.",
            }
        )
    if not binaries_unchanged:
        execution_failures.append(
            {
                "episode_id": None,
                "binary": None,
                "stage": "binary-integrity",
                "error": "A CLI or host binary SHA-256 changed during the run.",
            }
        )

    authoring = fixture.get("authoring")
    if not isinstance(authoring, dict):
        authoring = {}
    independently_authored = (
        authoring.get("independent_subagent") is True
        and fixture.get("execution_before_labels") is False
    )
    expected_recall_calls = 48
    execution_complete = (
        len(results) == 24
        and recall_attempted == expected_recall_calls
        and recall_completed == expected_recall_calls
        and sha_unchanged
        and binaries_unchanged
        and not execution_failures
    )
    continuation = None
    if resume_metadata is not None:
        continuation = {
            "source_report": str(args.resume_from),
            "source_report_sha256": resume_metadata["prior_report_sha256"],
            "identity_sidecar": str(args.resume_identity),
            "identity_sidecar_sha256": resume_metadata["identity_sha256"],
            "reused_completed_recall_calls": resume_metadata[
                "prior_recall_calls_completed"
            ],
            "continued_recall_calls": continued_calls,
            "policy": "Only source calls with recall_attempted=false were continued. Completed calls and raw packets were reused from the source report; an attempted but incomplete recall is refused.",
        }
    report = {
        "harness": "fresh_validation.py",
        "fixture_id": fixture["fixture_id"],
        "fixture_label": "independent synthetic" if independently_authored else "synthetic",
        "independent_authoring_evidence": {
            "authoring_independent_subagent": authoring.get("independent_subagent"),
            "execution_before_labels": fixture.get("execution_before_labels"),
        },
        "fixture_sha256_before": fixture_sha_before,
        "fixture_sha256_after": fixture_sha_after,
        "fixture_unchanged": sha_unchanged,
        "binary_sha256_before": binary_sha256_before,
        "binary_sha256_after": binary_sha256_after,
        "binaries_unchanged": binaries_unchanged,
        "seed": fixture["seed"],
        "baseline_cli": str(args.baseline_cli),
        "baseline_host": str(args.baseline_host),
        "candidate_cli": str(args.candidate_cli),
        "candidate_host": str(args.candidate_host),
        "episode_count": len(results),
        "slice_counts": dict(collections.Counter(result["slice"] for result in results)),
        "scope_note": "Paired synthetic cases are reported per episode; this is not an overall accuracy estimate.",
        "status": "complete" if execution_complete else "failed",
        "execution": {
            "status": "complete" if execution_complete else "failed",
            "recall_calls": {
                "expected": expected_recall_calls,
                "attempted": recall_attempted,
                "completed": recall_completed,
            },
            "failures": execution_failures,
        },
        "quality_summary": {
            "baseline_issues": baseline_issues,
            "candidate_issues": candidate_issues,
            "baseline_hard_violations": baseline_violations,
            "candidate_hard_violations": candidate_violations,
            "baseline_candidate_hard_violation_changes": hard_violation_changes,
            "candidate_new_unsupported_inclusions": candidate_new_unsupported,
            "baseline_candidate_changes": comparison_changes,
        },
        "continuation": continuation,
        "episodes": results,
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(
        json.dumps(
            {
                "status": report["status"],
                "fixture_id": report["fixture_id"],
                "fixture_sha256_before": fixture_sha_before,
                "fixture_sha256_after": fixture_sha_after,
                "binaries_unchanged": binaries_unchanged,
                "episodes": len(results),
                "recall_calls": {
                    "expected": expected_recall_calls,
                    "attempted": recall_attempted,
                    "completed": recall_completed,
                },
                "execution_failures": len(execution_failures),
                "candidate_quality_issue_episodes": len(candidate_issues),
                "candidate_hard_violations": len(candidate_violations),
                "new_candidate_hard_violations": sum(
                    len(item["new_candidate_violations"])
                    for item in hard_violation_changes
                ),
                "output": str(output_path),
            },
            ensure_ascii=False,
        )
    )
    return 0 if execution_complete else 1


if __name__ == "__main__":
    raise SystemExit(main())
