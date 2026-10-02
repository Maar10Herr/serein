#!/usr/bin/env python3
"""Build pinned prior readers for schema compatibility tests."""

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
READERS = {
    "schema3": ("035bc3b71a0a6484a46f39925621fffd14c5739f", "SEREIN_BASELINE_BINARY"),
    "schema4": ("aa44d488d3bde9222ec97133407c45e8fb14c6b3", "SEREIN_SCHEMA4_BINARY"),
}


def verify_reader(executable, expected_schema, temporary):
    """Check actual schema support; both pinned readers share a release version."""
    data = temporary / f"probe-schema{expected_schema}"
    vault_id = "99999999-9999-4999-8999-999999999999"
    database = data / "vaults" / vault_id / "context.sqlite"
    database.parent.mkdir(parents=True)
    (data / "connections.json").write_text(json.dumps({
        "version": 1,
        "default_vault": vault_id,
        "connections": [{
            "source_id": "11111111-1111-4111-8111-111111111111",
            "vault_id": vault_id,
            "extension_id": "a" * 32,
            "browser": "chrome",
            "nonce_hash": "test-only",
            "expires_at": 4102444800,
            "paired": True,
            "adapters": [],
            "skill_install_requested": False,
            "skill_repository": None,
            "label": "Disposable reader build probe",
        }],
    }))
    request = json.dumps({
        "protocol": 1,
        "request_id": "22222222-2222-4222-8222-222222222222",
        "client": "generic",
        "vault": "default",
        "query": "telescope mirror",
        "facets": [],
        "scope": ["research"],
        "max_bytes": 4096,
        "budget_ms": 1000,
    }).encode()
    env = {**os.environ, "SEREIN_DATA_DIR": str(data),
           "SEREIN_INSTALL_HOME": str(data / "install")}

    def recall():
        return subprocess.run([str(executable), "recall", "--request-stdin", "--json"],
                              input=request, capture_output=True, env=env, timeout=10)

    recall()  # Empty source policy can refuse recall after creating the schema.
    if not database.is_file():
        raise SystemExit(f"Schema-{expected_schema} reader did not create its probe database.")
    with closing(sqlite3.connect(database)) as connection:
        actual_schema = connection.execute("PRAGMA user_version").fetchone()[0]
        if actual_schema != expected_schema:
            raise SystemExit(f"Pinned reader created schema {actual_schema}; expected {expected_schema}.")
        connection.execute("PRAGMA user_version=5")
        connection.commit()
    result = recall()
    if result.returncode == 0 or json.loads(result.stdout).get("error", {}).get("code") != "SCHEMA_TOO_NEW":
        raise SystemExit(f"Schema-{expected_schema} reader failed to reject schema 5.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / ".test-data/migration-readers")
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {}
    with tempfile.TemporaryDirectory(prefix="serein-readers-") as temporary:
        temporary = Path(temporary)
        for name, (revision, variable) in READERS.items():
            resolved = subprocess.check_output(
                ["git", "rev-parse", f"{revision}^{{commit}}"], cwd=ROOT, text=True
            ).strip()
            if resolved != revision:
                raise SystemExit(f"Pinned {name} revision did not resolve exactly.")
            source = temporary / name
            source.mkdir()
            # Both revisions have the same package names and versions. Reusing
            # a target tree can reuse local-package fingerprints from the other
            # checkout, especially with archived source timestamps.
            target = temporary / f"target-{name}"
            env = {**os.environ, "CARGO_TARGET_DIR": str(target)}
            archive = subprocess.check_output(
                ["git", "archive", revision, "Cargo.toml", "Cargo.lock", "crates"], cwd=ROOT
            )
            subprocess.run(["tar", "-xf", "-", "-C", str(source)], input=archive, check=True)
            command = ["cargo", "build", "-p", "serein-cli", "--release", "--locked"]
            if args.offline:
                command.append("--offline")
            subprocess.run(command, cwd=source, env=env, check=True)
            destination = output / name
            destination.mkdir(exist_ok=True)
            executable = "serein.exe" if os.name == "nt" else "serein"
            shutil.copy2(target / "release" / executable, destination / executable)
            verify_reader(destination / executable, int(name.removeprefix("schema")), temporary)
            manifest[variable] = str(destination / executable)
        (output / "readers.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if environment_file := os.environ.get("GITHUB_ENV"):
        with Path(environment_file).open("a") as environment:
            for variable, executable in manifest.items():
                environment.write(f"{variable}={executable}\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
