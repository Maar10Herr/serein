#!/usr/bin/env python3
"""Verify skill handoff and owned local connection files in isolated directories."""

import json
import hashlib
import os
import pathlib
import subprocess
import tempfile
import time
import uuid

root = pathlib.Path(__file__).resolve().parents[1]
cli = root / "target/release/serein"

with tempfile.TemporaryDirectory(
    prefix="serein-install-", dir=str(pathlib.Path(tempfile.gettempdir()).resolve())
) as tmp:
    env = {
        **os.environ,
        "SEREIN_DATA_DIR": tmp + "/data",
        "SEREIN_INSTALL_HOME": tmp + "/home",
        "HERMES_HOME": tmp + "/hermes-profile",
        "OPENCLAW_STATE_DIR": tmp + "/openclaw-profile",
    }

    def run(command, request):
        return json.loads(
            subprocess.check_output(
                [str(cli), command, "--request-stdin", "--json"],
                input=json.dumps(request).encode(),
                env=env,
            )
        )

    selected = ["claude-code", "codex", "opencode", "hermes", "openclaw", "generic"]
    ticket = {
        "protocol": 1,
        "source_id": str(uuid.uuid4()),
        "extension_id": json.loads(
            (root / "release/extension-identity.json").read_text()
        )["chrome_extension_id"],
        "browser": "chrome",
        "nonce": str(uuid.uuid4()) + str(uuid.uuid4()),
        "expires_at": int(time.time()) + 890,
        "adapters": selected,
        "label": "Synthetic adapter installation",
        "consent": True,
    }

    first = run("setup", ticket)
    second = run("setup", ticket)
    assert first["database_path"] == second["database_path"]
    assert second["selected_adapters"] == selected
    assert second["skill_installation"]["state"] == "not_requested"
    assert len(second["adapters"]) == 6
    assert not any(record.get("installed") is True for record in second["adapters"])

    # Simulate a skill installed by the native GitHub Skills CLI. Setup adds only
    # the per-machine executable pointer and leaves the downloaded SKILL.md alone.
    codex_dir = pathlib.Path(tmp) / "home/.codex/skills/serein-context"
    codex_dir.mkdir(parents=True)
    codex = codex_dir / "SKILL.md"
    original_skill = "---\nname: serein-context\n---\nUser's installed skill.\n"
    codex.write_text(original_skill)
    configured = run("setup", ticket)
    assert codex.read_text() == original_skill
    connection = codex_dir / "references/connection.md"
    assert connection.is_file()
    assert any(
        item["id"] == "codex" and item["state"] == "configured"
        for item in configured["connections"]
    )

    connection.write_text(connection.read_text() + "\nUser customization preserved.\n")
    again = run("setup", ticket)
    assert any(
        item["id"] == "codex" and item["state"] == "user_edits_preserved"
        for item in again["connections"]
    )
    assert codex.read_text() == original_skill

    # A release upgrade must replace our old host path without overwriting a
    # registration the user has edited since its ownership receipt was written.
    manifest_path = pathlib.Path(first["manifest"])
    receipt_path = pathlib.Path(tmp) / "data/receipts/native-chrome.json"
    old_manifest = json.loads(manifest_path.read_text())
    old_manifest["path"] = str(pathlib.Path(tmp) / "previous-version/serein-host")
    old_bytes = json.dumps(old_manifest).encode()
    manifest_path.write_bytes(old_bytes)
    receipt = json.loads(receipt_path.read_text())
    receipt["sha256"] = hashlib.sha256(old_bytes).hexdigest()
    receipt_path.write_text(json.dumps(receipt))
    upgraded = run("setup", ticket)
    installed_bytes = manifest_path.read_bytes()
    assert json.loads(installed_bytes)["path"] != old_manifest["path"]
    assert upgraded["database_path"] == first["database_path"]

    customized = json.loads(installed_bytes)
    customized["description"] = "User-managed registration"
    customized_bytes = json.dumps(customized).encode()
    manifest_path.write_bytes(customized_bytes)
    try:
        run("setup", ticket)
        raise AssertionError("setup replaced a user-modified registration")
    except subprocess.CalledProcessError as exc:
        assert json.loads(exc.output)["error"]["code"] == "ACCESS_DENIED"
    assert manifest_path.read_bytes() == customized_bytes
    manifest_path.write_bytes(installed_bytes)

    result = run("uninstall", {"consent": True, "erase_vaults": False})
    assert codex.exists()
    assert connection.exists()
    assert pathlib.Path(first["database_path"]).exists()
    assert not pathlib.Path(first["manifest"]).exists()
    assert str(connection) in result["preserved"]

    report = {
        "checks": [
            "selected adapters remain distinct from detected installation state",
            "setup does not copy a skill without explicit native installer request",
            "existing GitHub-installed SKILL.md is preserved",
            "local executable pointer is created and user edits are preserved",
            "owned browser registration upgrades while retaining the vault",
            "user-modified browser registration is preserved and rejected",
            "native manifest removed",
            "vault retained by explicit choice",
        ],
        "real_assistant_discovery_and_execution": False,
        "native_skills_cli_execution": False,
    }
    (root / "docs/install-test-results.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    print(json.dumps(report, indent=2))
