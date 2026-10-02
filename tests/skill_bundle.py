#!/usr/bin/env python3
"""Exercise an installed skill and its one-shot offline recall path."""

from __future__ import annotations

import datetime
import errno
import json
import os
import pathlib
import platform
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
SKILL = pathlib.Path(os.environ.get("SEREIN_SKILL_PATH", str(ROOT / "skills/serein-context")))
MAX_CONTEXT_RECORDS = 6


def checked_run(
    command: list[str],
    *,
    env: dict[str, str],
    input_bytes: bytes | None = None,
    timeout: int,
) -> subprocess.CompletedProcess[bytes]:
    result = subprocess.run(
        command,
        input=input_bytes,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
        timeout=timeout,
    )
    assert result.returncode == 0, (
        command,
        result.returncode,
        result.stdout.decode(errors="replace"),
        result.stderr.decode(errors="replace"),
    )
    return result


def assert_os_network_denial() -> None:
    """Verify the caller's OS sandbox denies a loopback UDP operation."""
    probe = None
    stage = "socket creation"
    try:
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        stage = "connect"
        probe.connect(("127.0.0.1", 9))
        stage = "send"
        probe.send(b"serein-offline-probe")
    except OSError as error:
        assert error.errno in {errno.EPERM, errno.EACCES}, (
            f"loopback UDP {stage} was not denied by the OS sandbox",
            error.errno,
            str(error),
        )
    else:
        raise AssertionError("OS sandbox allowed a loopback UDP connect/send")
    finally:
        if probe is not None:
            probe.close()


def main() -> None:
    offline_setting = os.environ.get("SEREIN_REQUIRE_OFFLINE", "")
    assert offline_setting in {"", "1"}, "SEREIN_REQUIRE_OFFLINE accepts only 1"
    require_offline = offline_setting == "1"
    if require_offline:
        assert_os_network_denial()

    if (platform.system(), platform.machine().lower()) not in {
        ("Darwin", "arm64"), ("Darwin", "aarch64")
    }:
        if require_offline:
            raise AssertionError(
                "SEREIN_REQUIRE_OFFLINE=1 needs the bundled macOS Apple-silicon skill runtime"
            )
        print("SKIP: bundled installed-skill helper test requires macOS Apple silicon")
        return

    with tempfile.TemporaryDirectory(
        prefix="serein-skill-test-",
        dir=str(pathlib.Path(tempfile.gettempdir()).resolve()),
    ) as temporary:
        base = pathlib.Path(temporary)
        installed = base / "installed skills" / "serein-context"
        shutil.copytree(SKILL, installed)
        for binary in ("serein", "serein-host"):
            (installed / "runtime/macos-arm64" / binary).chmod(0o600)
        home = base / "user home"
        home.mkdir()
        data = base / "application data"
        scratch = base / "temporary files"
        scratch.mkdir()
        env = {
            **os.environ,
            "TMPDIR": str(scratch),
            "SEREIN_INSTALL_HOME": str(home),
            "SEREIN_DATA_DIR": str(data),
        }
        source = str(uuid.uuid4())
        extension_id = "a" * 32
        nonce = str(uuid.uuid4()) + str(uuid.uuid4())
        ticket = {
            "protocol": 1,
            "source_id": source,
            "extension_id": extension_id,
            "browser": "chrome",
            "nonce": nonce,
            "expires_at": int(time.time()) + 900,
            "adapters": ["generic"],
            "label": "Skill bundle test",
            "consent": True,
            "install_skills": False,
        }
        connected = checked_run(
            ["sh", str(installed / "scripts/connect.sh")],
            input_bytes=json.dumps(ticket).encode(),
            env=env,
            timeout=30,
        )
        setup = json.loads(connected.stdout)
        assert setup["status"] == "ok", setup
        assert setup["database_path"].startswith(str(data / "vaults")), setup
        assert pathlib.Path(setup["database_path"]).exists(), setup
        manifest_path = pathlib.Path(setup["manifest"])
        assert manifest_path.is_file(), setup
        manifest = json.loads(manifest_path.read_text())
        assert str(home) in manifest["path"]
        assert "installed skills" not in manifest["path"]
        assert "chrome-extension://" + extension_id + "/" in manifest["allowed_origins"]
        assert manifest["type"] == "stdio"
        assert os.access(installed / "runtime/macos-arm64/serein", os.X_OK)

        # Invoke the exact helper registered by setup, using Chrome's native
        # messaging origin and length-prefixed JSON frame.
        registered_host = pathlib.Path(manifest["path"])
        assert registered_host.is_file(), manifest
        assert home in registered_host.parents, manifest
        origin = f"chrome-extension://{extension_id}/"

        def native(op: str, payload: dict[str, object]) -> dict[str, object]:
            request_id = str(uuid.uuid4())
            envelope = {
                "protocol": 1,
                "request_id": request_id,
                "source_id": source,
                "op": op,
                "capture_epoch": 2,
                "payload": payload,
            }
            body = json.dumps(envelope, ensure_ascii=False, separators=(",", ":")).encode()
            frame = struct.pack("=I", len(body)) + body
            result = checked_run(
                [str(registered_host), origin], env=env, input_bytes=frame, timeout=10
            )
            assert len(result.stdout) >= 4, (op, result.stdout, result.stderr)
            length = struct.unpack("=I", result.stdout[:4])[0]
            assert length <= 256 * 1024 and len(result.stdout) == length + 4, (
                op,
                len(result.stdout),
                length,
            )
            response = json.loads(result.stdout[4:])
            assert response["request_id"] == request_id, response
            assert response.get("status") != "error", response
            return response

        hello = native("hello", {"nonce": nonce})
        assert hello["status"] == "ok", hello
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
        assert policy["policy"]["recall_enabled"] is True, policy

        def capture() -> dict[str, object]:
            event_id = str(uuid.uuid4())
            event = {
                "event_id": event_id,
                "visit_id": str(uuid.uuid4()),
                "site_key": "lighting.example.org",
                "site_epoch": 2,
                "observed_at": datetime.datetime.now(datetime.timezone.utc)
                .isoformat(timespec="seconds")
                .replace("+00:00", "Z"),
                "kind": "search",
                "title": "Desk lamp reading room lighting",
                "search_query": "desk lamp reading room lighting",
                "foreground_seconds": 30,
            }
            result = native("ingest", {"events": [event]})
            assert result["acknowledged_ids"] == [event_id], result
            assert result["rejected"] == [], result
            return result

        first_capture = capture()
        assert first_capture["model_available"] is True, first_capture
        assert first_capture["index_mode"] == "hybrid", first_capture
        pending_before = int(first_capture["pending_atoms"])
        assert pending_before >= 1, first_capture

        request = {
            "protocol": 1,
            "request_id": str(uuid.uuid4()),
            "client": "generic",
            "vault": "default",
            "query": "desk lamp reading room lighting",
            "facets": ["desk lamp", "reading room"],
            "scope": ["research"],
            "max_bytes": 4096,
            "budget_ms": 2000,
        }

        def installed_recall() -> tuple[dict[str, object], int]:
            request["request_id"] = str(uuid.uuid4())
            result = checked_run(
                ["sh", str(installed / "scripts/recall.sh")],
                input_bytes=json.dumps(request, ensure_ascii=False).encode(),
                env=env,
                timeout=20,
            )
            packet_bytes = result.stdout[:-1] if result.stdout.endswith(b"\n") else result.stdout
            assert len(result.stdout) - len(packet_bytes) <= 1, result.stdout[-16:]
            assert len(packet_bytes) <= request["max_bytes"], len(packet_bytes)
            response = json.loads(packet_bytes)
            assert response["request_id"] == request["request_id"], response
            assert response["index"]["mode"] == "hybrid", response
            assert response["index"]["pending_atoms"] == 0, response
            records = response["context"]
            assert records and len(records) <= MAX_CONTEXT_RECORDS, response
            assert any(
                record["state"] == "observed"
                and "lighting.example.org" in record["text"]
                and "desk lamp" in record["text"].lower()
                for record in records
            ), response
            assert all(
                isinstance(record.get("limits"), list)
                and isinstance(record.get("id"), str)
                and isinstance(record.get("text"), str)
                for record in records
            ), response
            return response, len(result.stdout)

        first_recall, first_size = installed_recall()
        assert pending_before > first_recall["index"]["pending_atoms"], first_recall

        # A second independent capture and installed recall both finish as
        # one-shot subprocesses; no resident helper is needed between calls.
        second_capture = capture()
        assert second_capture["pending_atoms"] == 0, second_capture
        second_recall, second_size = installed_recall()
        assert second_recall["context"], second_recall
        print(
            "PASS: relocated skill connected; registered native helper completed hello, policy, and two captures; "
            f"installed recall indexed pending evidence on read and returned bounded hybrid packets ({first_size}, {second_size} bytes)"
        )
        if require_offline:
            print("PASS: OS sandbox denied loopback UDP connect/send; no external network probe was used")


if __name__ == "__main__":
    main()
