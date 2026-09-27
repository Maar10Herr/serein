#!/usr/bin/env python3
"""Exercise link setup from an installed skill at a path containing spaces."""

from __future__ import annotations

import json
import os
import pathlib
import platform
import shutil
import subprocess
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
SKILL = pathlib.Path(os.environ.get("SEREIN_SKILL_PATH", str(ROOT / "skills/serein-context")))


def main() -> None:
    if (platform.system(), platform.machine().lower()) not in {
        ("Darwin", "arm64"), ("Darwin", "aarch64")
    }:
        print("SKIP: bundled helper currently targets macOS Apple silicon")
        return
    with tempfile.TemporaryDirectory(prefix="serein-skill-test-", dir=str(pathlib.Path(tempfile.gettempdir()).resolve())) as temporary:
        base = pathlib.Path(temporary)
        installed = base / "installed skills" / "serein-context"
        shutil.copytree(SKILL, installed)
        for binary in ("serein", "serein-host"):
            (installed / "runtime/macos-arm64" / binary).chmod(0o600)
        home = base / "user home"
        home.mkdir()
        data = base / "application data"
        env = {
            **os.environ,
            "SEREIN_INSTALL_HOME": str(home),
            "SEREIN_DATA_DIR": str(data),
        }
        source = str(uuid.uuid4())
        ticket = {
            "protocol": 1,
            "source_id": source,
            "extension_id": "a" * 32,
            "browser": "chrome",
            "nonce": str(uuid.uuid4()) + str(uuid.uuid4()),
            "expires_at": int(time.time()) + 900,
            "adapters": ["generic"],
            "label": "Skill bundle test",
            "consent": True,
            "install_skills": False,
        }
        result = subprocess.run(
            ["sh", str(installed / "scripts/connect.sh")],
            input=json.dumps(ticket).encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env=env,
            timeout=30,
        )
        assert result.returncode == 0, (result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace"))
        setup = json.loads(result.stdout)
        assert setup["status"] == "ok", setup
        assert setup["database_path"].startswith(str(data / "vaults")), setup
        assert pathlib.Path(setup["database_path"]).exists(), setup
        manifest = json.loads(pathlib.Path(setup["manifest"]).read_text())
        assert str(home) in manifest["path"]
        assert "installed skills" not in manifest["path"]
        assert (data / "models/current/manifest.json").is_file()
        assert os.access(installed / "runtime/macos-arm64/serein", os.X_OK)
        print("PASS: relocated skill connected with bundled helper; database and registered host live outside the skill")


if __name__ == "__main__":
    main()
