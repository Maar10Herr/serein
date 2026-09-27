#!/usr/bin/env python3
"""Copy the built macOS Apple silicon runtime into the installable skill."""

from __future__ import annotations

import hashlib
import json
import pathlib
import platform
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
VERSION = (ROOT / "Cargo.toml").read_text().split('version = "', 1)[1].split('"', 1)[0]
RUNTIME = ROOT / "skills/serein-context/runtime/macos-arm64"
MODEL_FILES = (
    "manifest.json",
    "tokenizer.json",
    "weights.i8",
    "scales.f32",
    "LICENSE-2.0.txt",
    "NOTICE.md",
)


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    if (platform.system(), platform.machine().lower()) not in {
        ("Darwin", "arm64"), ("Darwin", "aarch64")
    }:
        raise SystemExit("The bundled runtime build currently supports macOS Apple silicon only.")
    binary = ROOT / "target/release/serein"
    host = ROOT / "target/release/serein-host"
    if not binary.is_file() or not host.is_file():
        raise SystemExit("Build both release binaries before bundling the skill.")
    built_version = json.loads(subprocess.check_output([binary, "--version"], timeout=5))["version"]
    if built_version != VERSION:
        raise SystemExit(f"CLI version {built_version} does not match workspace {VERSION}.")
    RUNTIME.mkdir(parents=True, exist_ok=True)
    for source in (binary, host):
        shutil.copy2(source, RUNTIME / source.name)
    model_source = ROOT / "models/pack"
    (RUNTIME / "model").mkdir(exist_ok=True)
    for name in MODEL_FILES:
        shutil.copy2(model_source / name, RUNTIME / "model" / name)
    license_source = ROOT / "release/dependency-licenses"
    if not license_source.is_dir():
        raise SystemExit("Dependency license inventory is missing.")
    for source in license_source.rglob("*"):
        if source.is_symlink():
            raise SystemExit(f"Unexpected symlink in license inventory: {source}")
        if source.is_file():
            destination = RUNTIME / "dependency-licenses" / source.relative_to(license_source)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
    shutil.copy2(ROOT / "docs/THIRD_PARTY_LICENSES.json", RUNTIME / "THIRD_PARTY_LICENSES.json")
    files = sorted(path for path in RUNTIME.rglob("*") if path.is_file() and path.name != "SHA256SUMS")
    (RUNTIME / "SHA256SUMS").write_text(
        "".join(f"{digest(path)}  {path.relative_to(RUNTIME).as_posix()}\n" for path in files),
        encoding="utf-8",
    )
    print(f"Bundled Serein {VERSION} for macOS Apple silicon ({len(files)} files).")


if __name__ == "__main__":
    main()
