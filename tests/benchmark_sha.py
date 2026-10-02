#!/usr/bin/env python3
"""Paired warm-cache CLI recall benchmark for two local Serein builds.

The fixture is synthetic, indexed once with the baseline, then copied as a
closed data directory to the candidate so both builds recall the same IDs,
model pack, and SQLite state. No browser profile or native user registration is
read or changed. Each recall runs in its own short-lived measurement worker;
the worker times the CLI process and reads that process's peak RSS.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import platform
import resource
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import time
import uuid


def uid() -> str:
    return str(uuid.uuid4())


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run_json(command: list[str], *, env: dict[str, str], payload: dict | None = None,
             timeout: float = 30) -> dict:
    result = subprocess.run(
        command,
        input=json.dumps(payload, separators=(",", ":")).encode() if payload is not None else None,
        capture_output=True,
        env=env,
        check=False,
        timeout=timeout,
    )
    if result.returncode:
        raise RuntimeError(
            f"Command exited {result.returncode}: {command[1:]}\n"
            f"stdout={result.stdout[-1000:]!r}\nstderr={result.stderr[-1000:]!r}"
        )
    try:
        return json.loads(result.stdout)
    except Exception as error:
        raise RuntimeError(
            f"Command returned invalid JSON: {command[1:]}\n"
            f"stdout={result.stdout[-1000:]!r}\nstderr={result.stderr[-1000:]!r}"
        ) from error


def worker(cli: pathlib.Path) -> int:
    """Measure one CLI process and report its own peak child RSS."""
    request = sys.stdin.buffer.read()
    started = time.perf_counter()
    result = subprocess.run(
        [str(cli), "recall", "--request-stdin", "--json"],
        input=request,
        capture_output=True,
        env=os.environ.copy(),
        timeout=10,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    # macOS reports bytes; Linux and the supported CI Linux runners report KiB.
    peak_bytes = int(peak if sys.platform == "darwin" else peak * 1024)
    report = {
        "returncode": result.returncode,
        "stdout": result.stdout.decode("utf-8", errors="replace"),
        "stderr": result.stderr.decode("utf-8", errors="replace"),
        "elapsed_ms": elapsed_ms,
        "peak_rss_bytes": peak_bytes,
    }
    sys.stdout.write(json.dumps(report, separators=(",", ":")))
    return 0


def invoke_worker(cli: pathlib.Path, request: dict, env: dict[str, str]) -> tuple[dict, float, int]:
    result = subprocess.run(
        [
            sys.executable,
            str(pathlib.Path(__file__).resolve()),
            "--worker",
            str(cli),
            "recall",
        ],
        input=json.dumps(request, separators=(",", ":")).encode(),
        capture_output=True,
        env=env,
        timeout=15,
        check=True,
    )
    measurement = json.loads(result.stdout)
    if measurement["returncode"]:
        raise RuntimeError(
            f"{cli.name} recall failed ({measurement['returncode']}): "
            f"{measurement['stderr'][-1000:]} {measurement['stdout'][-1000:]}"
        )
    packet = json.loads(measurement["stdout"])
    return packet, float(measurement["elapsed_ms"]), int(measurement["peak_rss_bytes"])


def native(host: pathlib.Path, extension_id: str, env: dict[str, str],
           source: str, op: str, payload: dict, *, epoch: int = 2) -> dict:
    message = json.dumps(
        {
            "protocol": 1,
            "request_id": uid(),
            "source_id": source,
            "op": op,
            "capture_epoch": epoch,
            "payload": payload,
        },
        separators=(",", ":"),
    ).encode()
    framed = struct.pack("<I", len(message)) + message
    result = subprocess.run(
        [str(host), f"chrome-extension://{extension_id}/"],
        input=framed,
        capture_output=True,
        env=env,
        timeout=10,
        check=True,
    )
    if len(result.stdout) < 4:
        raise RuntimeError(f"{op} returned a truncated native frame: {result.stderr!r}")
    length = struct.unpack("<I", result.stdout[:4])[0]
    packet = json.loads(result.stdout[4:4 + length])
    if len(result.stdout) != length + 4:
        raise RuntimeError(f"{op} returned an invalid native frame length")
    return packet


def fixture() -> list[dict]:
    observed_at = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
    events = []
    for index in range(800):
        events.append(
            {
                "event_id": uid(),
                "visit_id": uid(),
                "site_key": f"research{index % 20:02d}.example.org",
                "site_epoch": 0,
                "observed_at": observed_at,
                "kind": "search",
                "title": f"Desk lamp research option {index:04d} review specifications",
                "search_query": f"desk lamp research option {index:04d} review",
                "foreground_seconds": 30,
            }
        )
    return events


def run_setup(cli: pathlib.Path, env: dict[str, str], extension_id: str,
              source: str, nonce: str) -> dict:
    setup = {
        "protocol": 1,
        "source_id": source,
        "extension_id": extension_id,
        "browser": "chrome",
        "nonce": nonce,
        "expires_at": int(time.time()) + 600,
        "adapters": ["generic"],
        "label": "Synthetic paired benchmark",
        "consent": True,
    }
    return run_json(
        [str(cli), "setup", "--request-stdin", "--json"], env=env, payload=setup
    )


def prepare_fixture(cli: pathlib.Path, host: pathlib.Path, data_root: pathlib.Path,
                    home_root: pathlib.Path, extension_id: str) -> dict:
    data_root.mkdir(parents=True)
    home_root.mkdir(parents=True)
    env = {
        **os.environ,
        "SEREIN_DATA_DIR": str(data_root),
        "SEREIN_INSTALL_HOME": str(home_root),
    }
    source, nonce = uid(), uid() + uid()
    setup_result = run_setup(cli, env, extension_id, source, nonce)
    if setup_result.get("status") not in {"ok", "partial"}:
        raise RuntimeError(f"Synthetic CLI setup did not complete: {setup_result}")
    native(host, extension_id, env, source, "hello", {"nonce": nonce})
    native(
        host,
        extension_id,
        env,
        source,
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
    events = fixture()
    for offset in range(0, len(events), 32):
        batch = events[offset:offset + 32]
        result = native(host, extension_id, env, source, "ingest", {"events": batch})
        if len(result.get("acknowledged_ids", [])) != len(batch):
            raise RuntimeError(f"Only part of synthetic batch was acknowledged: {result}")
    before = native(host, extension_id, env, source, "status", {})
    if before.get("atoms") != 800:
        raise RuntimeError(f"Expected exactly 800 synthetic atoms, got: {before}")

    refreshes = []
    for _ in range(100):
        refreshed = run_json(
            [str(cli), "refresh", "--budget-ms", "5000", "--json"],
            env=env,
            timeout=20,
        )
        refreshes.append(refreshed)
        if refreshed.get("pending_atoms") == 0:
            break
    else:
        raise RuntimeError(f"Synthetic index did not finish: {refreshes[-1]}")
    if refreshed.get("mode") != "hybrid":
        raise RuntimeError(f"800-atom fixture did not use the model index: {refreshed}")
    after = native(host, extension_id, env, source, "status", {})
    if after.get("atoms") != 800 or after.get("pending_atoms") != 0:
        raise RuntimeError(f"Fixture is not fully indexed: {after}")

    dbs = list(data_root.glob("vaults/*/context.sqlite"))
    if len(dbs) != 1:
        raise RuntimeError(f"Expected one synthetic vault database, found {dbs}")
    return {
        "env": env,
        "source_id": source,
        "event_count": len(events),
        "fixture_sha256": hashlib.sha256(
            json.dumps(events, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "setup_status": setup_result.get("status"),
        "pre_index_status": before,
        "index_refreshes": [
            {key: item.get(key) for key in ("status", "mode", "processed", "pending_atoms")}
            for item in refreshes
        ],
        "post_index_status": after,
    }


def ordered_records(packet: dict) -> dict:
    return {
        "status": packet.get("status"),
        "index": packet.get("index"),
        "generation": packet.get("generation"),
        "ordered_context": [
            {
                "id": row.get("id"),
                "text": row.get("text"),
                "state": row.get("state"),
            }
            for row in packet.get("context", [])
        ],
        "context_records": packet.get("context", []),
        "ordered_alternatives": [
            {
                "id": row.get("id"),
                "text": row.get("text"),
                "state": row.get("state"),
            }
            for row in packet.get("alternatives", [])
        ],
        "alternative_records": packet.get("alternatives", []),
        "warnings": packet.get("warnings", []),
    }


def summary(values: list[float]) -> dict:
    ordered = sorted(values)
    q1, q3 = statistics.quantiles(ordered, n=4, method="inclusive")[::2]
    lower, upper = q1 - 1.5 * (q3 - q1), q3 + 1.5 * (q3 - q1)
    outlier_indices = [
        index for index, value in enumerate(values) if value < lower or value > upper
    ]
    return {
        "samples": len(values),
        "median_ms": statistics.median(values),
        "p95_ms_nearest_lower_rank": ordered[int(0.95 * (len(ordered) - 1))],
        "q1_ms_inclusive": q1,
        "q3_ms_inclusive": q3,
        "tukey_outlier_pair_indices_zero_based": outlier_indices,
        "raw_pair_samples_ms": values,
    }


def database_sizes(data_root: pathlib.Path) -> dict:
    dbs = list(data_root.glob("vaults/*/context.sqlite"))
    if len(dbs) != 1:
        raise RuntimeError(f"Expected one vault database: {dbs}")
    db = dbs[0]
    wal = pathlib.Path(str(db) + "-wal")
    shm = pathlib.Path(str(db) + "-shm")
    return {
        "database_bytes": db.stat().st_size,
        "wal_bytes": wal.stat().st_size if wal.exists() else 0,
        "shm_bytes": shm.stat().st_size if shm.exists() else 0,
        "database_plus_wal_bytes": db.stat().st_size + (wal.stat().st_size if wal.exists() else 0),
    }


def execute(args: argparse.Namespace) -> dict:
    baseline_cli = args.baseline_cli.resolve()
    baseline_host = args.baseline_host.resolve()
    candidate_cli = args.candidate_cli.resolve()
    candidate_host = args.candidate_host.resolve()
    model = args.model.resolve()
    extension_file = args.extension_identity.resolve()
    for path in (baseline_cli, baseline_host, candidate_cli, candidate_host):
        if not path.is_file() or not os.access(path, os.X_OK):
            raise RuntimeError(f"Expected an executable binary: {path.name}")
    if not (model / "manifest.json").is_file():
        raise RuntimeError(f"Model pack is incomplete: {model}")
    extension_id = json.loads(extension_file.read_text())["chrome_extension_id"]
    if len(extension_id) != 32:
        raise RuntimeError("Expected the packaged 32-character Chrome extension ID")

    with tempfile.TemporaryDirectory(prefix="serein-paired-cli-") as temporary:
        tmp = pathlib.Path(temporary).resolve()
        runtime = tmp / "runtime"
        baseline_runtime = runtime / "baseline"
        candidate_runtime = runtime / "candidate"
        for directory in (baseline_runtime, candidate_runtime):
            directory.mkdir(parents=True)
            shutil.copy2(model / "manifest.json", directory / "manifest.json")
            shutil.copytree(model, directory / "model")
        baseline_cli_copy = baseline_runtime / "serein"
        baseline_host_copy = baseline_runtime / "serein-host"
        candidate_cli_copy = candidate_runtime / "serein"
        candidate_host_copy = candidate_runtime / "serein-host"
        for source, destination in (
            (baseline_cli, baseline_cli_copy),
            (baseline_host, baseline_host_copy),
            (candidate_cli, candidate_cli_copy),
            (candidate_host, candidate_host_copy),
        ):
            shutil.copy2(source, destination)
            destination.chmod(destination.stat().st_mode | 0o100)

        roots = {"baseline": tmp / "data-baseline", "candidate": tmp / "data-candidate"}
        homes = {"baseline": tmp / "home-baseline", "candidate": tmp / "home-candidate"}
        baseline_fixture = prepare_fixture(
            baseline_cli_copy,
            baseline_host_copy,
            roots["baseline"],
            homes["baseline"],
            extension_id,
        )
        # A byte-for-byte clone gives the candidate identical salts, source/vault IDs,
        # event IDs, index state, receipts, and verified model assets.
        shutil.copytree(roots["baseline"], roots["candidate"])
        homes["candidate"].mkdir()
        environments = {
            label: {
                **os.environ,
                "SEREIN_DATA_DIR": str(root),
                "SEREIN_INSTALL_HOME": str(homes[label]),
            }
            for label, root in roots.items()
        }

        request = {
            "protocol": 1,
            "request_id": uid(),
            "client": "generic",
            "vault": "default",
            "query": "desk lamp research review",
            "facets": ["desk lamp"],
            "scope": ["research"],
            "max_bytes": 4096,
            "budget_ms": 2000,
        }
        # Warm both process/model paths and verify the comparison before sampling.
        warm_packets = {}
        for label, cli in (("baseline", baseline_cli_copy), ("candidate", candidate_cli_copy)):
            warm_packets[label], _, _ = invoke_worker(cli, request, environments[label])
        if ordered_records(warm_packets["baseline"]) != ordered_records(warm_packets["candidate"]):
            raise RuntimeError("Warm-up output mismatch; no timing report was written")

        pair_records = []
        durations = {"baseline": [], "candidate": []}
        peak_rss = {"baseline": [], "candidate": []}
        for index in range(25):
            request_id = uid()
            pair_request = {**request, "request_id": request_id}
            order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
            packets, timings, peaks = {}, {}, {}
            for label in order:
                cli = baseline_cli_copy if label == "baseline" else candidate_cli_copy
                packets[label], timings[label], peaks[label] = invoke_worker(
                    cli, pair_request, environments[label]
                )
                durations[label].append(timings[label])
                peak_rss[label].append(peaks[label])
            equal = ordered_records(packets["baseline"]) == ordered_records(packets["candidate"])
            pair_records.append(
                {
                    "pair_index": index,
                    "order": list(order),
                    "matching_ordered_ids_text_state_and_packet_fields": equal,
                    "baseline_elapsed_ms": timings["baseline"],
                    "candidate_elapsed_ms": timings["candidate"],
                    "baseline_peak_rss_bytes": peaks["baseline"],
                    "candidate_peak_rss_bytes": peaks["candidate"],
                    "baseline_output": ordered_records(packets["baseline"]),
                    "candidate_output": ordered_records(packets["candidate"]),
                }
            )
        mismatches = [entry["pair_index"] for entry in pair_records
                      if not entry["matching_ordered_ids_text_state_and_packet_fields"]]
        if mismatches:
            raise RuntimeError(f"Baseline/candidate recall outputs differ in pairs: {mismatches}")

        return {
            "status": "pass",
            "platform": platform.system(),
            "architecture": platform.machine(),
            "build_profile": "release",
            "fixture": baseline_fixture,
            "model_manifest_sha256": sha256(model / "manifest.json"),
            "model_payload_sha256": {
                name: sha256(model / name)
                for name in ("weights.i8", "scales.f32", "tokenizer.json")
            },
            "baseline_binary_sha256": {
                "serein": sha256(baseline_cli), "serein_host": sha256(baseline_host)
            },
            "candidate_binary_sha256": {
                "serein": sha256(candidate_cli), "serein_host": sha256(candidate_host)
            },
            "fixture_atoms": 800,
            "indexed_before_pairs": True,
            "cache_state": "warm after full index and one discarded recall from each build; no forced OS cache eviction",
            "timing_scope": "per-CLI-process recall wall time, including process launch; 25 paired samples with alternating AB/BA order",
            "rss_scope": "ru_maxrss for the single CLI child measured in an isolated worker; bytes normalized for the current OS",
            "recall_request": {
                "query": request["query"],
                "facets": request["facets"],
                "scope": request["scope"],
                "max_bytes": request["max_bytes"],
                "budget_ms": request["budget_ms"],
            },
            "outputs_identical": True,
            "baseline": {
                "recall": summary(durations["baseline"]),
                "peak_rss_bytes_by_pair": peak_rss["baseline"],
                "storage_after_pairs": database_sizes(roots["baseline"]),
            },
            "candidate": {
                "recall": summary(durations["candidate"]),
                "peak_rss_bytes_by_pair": peak_rss["candidate"],
                "storage_after_pairs": database_sizes(roots["candidate"]),
            },
            "pairs": pair_records,
            "limitations": [
                "Synthetic fixture and this host are not a representative user workload or a reference laptop.",
                "Warm-cache samples do not estimate true cold-cache latency.",
                "The result does not measure browser or assistant integration behavior.",
            ],
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", type=pathlib.Path)
    parser.add_argument("--baseline-host", type=pathlib.Path)
    parser.add_argument("--candidate-cli", type=pathlib.Path)
    parser.add_argument("--candidate-host", type=pathlib.Path)
    parser.add_argument("--model", type=pathlib.Path)
    parser.add_argument("--extension-identity", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--worker", nargs=2, metavar=("CLI", "MODE"))
    args = parser.parse_args()
    if args.worker:
        cli, mode = args.worker
        if mode != "recall":
            parser.error("worker mode must be recall")
        return worker(pathlib.Path(cli))
    required = (
        args.baseline_cli, args.baseline_host, args.candidate_cli,
        args.candidate_host, args.model, args.extension_identity, args.output,
    )
    if any(value is None for value in required):
        parser.error("supply baseline/candidate CLI and host, model, extension identity, and output")
    report = execute(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps({
        "status": report["status"],
        "fixture_atoms": report["fixture_atoms"],
        "outputs_identical": report["outputs_identical"],
        "baseline_median_ms": report["baseline"]["recall"]["median_ms"],
        "candidate_median_ms": report["candidate"]["recall"]["median_ms"],
        "output": str(args.output),
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
