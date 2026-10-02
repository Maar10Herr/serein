#!/usr/bin/env python3
"""Copy the built macOS Apple silicon runtime into the installable skill."""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import subprocess
import tempfile
import uuid

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
MODEL_SOURCE_ONLY_FILES = frozenset({"parity.json"})
LEGAL_NOTICE_NAME = re.compile(
    r"^(?:LICENSE|COPYING|NOTICE|COPYRIGHT)(?:[._-][A-Za-z0-9.+_-]+)?$",
    re.IGNORECASE,
)
UNSAFE_NOTICE_SUFFIXES = frozenset(
    {".bak", ".backup", ".cer", ".crt", ".db", ".der", ".env", ".jks", ".key", ".log", ".p12", ".pem", ".pfx", ".secret", ".sqlite", ".sqlite3", ".swp", ".temp", ".tmp", ".token"}
)
UNSAFE_PATH_PARTS = frozenset(
    {"credential", "credentials", "key", "keys", "private", "secret", "secrets", "temp", "temporary", "token", "tokens", "tmp"}
)


def digest(path: pathlib.Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def _require_regular_file(path: pathlib.Path, description: str) -> pathlib.Path:
    if path.is_symlink() or not path.is_file():
        raise SystemExit(f"Missing regular {description}: {path}")
    return path


def _tree_files(directory: pathlib.Path, description: str) -> list[pathlib.Path]:
    if directory.is_symlink() or not directory.is_dir():
        raise SystemExit(f"Missing {description} directory: {directory}")
    files = []
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise SystemExit(f"Unexpected symlink in {description}: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise SystemExit(f"Unexpected non-file in {description}: {path}")
        files.append(path)
    return files


def assert_no_runtime_workdirs(runtime: pathlib.Path = RUNTIME) -> None:
    """Refuse to build or package while a recoverable stage/backup is present."""
    parent = pathlib.Path(runtime).parent
    if not parent.is_dir():
        return
    prefixes = (f".{pathlib.Path(runtime).name}.stage-", f".{pathlib.Path(runtime).name}.backup-")
    leftovers = sorted(path for path in parent.iterdir() if path.name.startswith(prefixes))
    if leftovers:
        rendered = ", ".join(str(path) for path in leftovers[:4])
        suffix = " …" if len(leftovers) > 4 else ""
        raise SystemExit(
            f"Incomplete skill runtime staging or backup remains; preserving it and "
            f"refusing to continue: {rendered}{suffix}"
        )


def runtime_source_files(
    root: pathlib.Path = ROOT,
    *,
    binary: pathlib.Path | None = None,
    host: pathlib.Path | None = None,
    model_source: pathlib.Path | None = None,
    license_source: pathlib.Path | None = None,
    third_party_licenses: pathlib.Path | None = None,
) -> dict[str, pathlib.Path]:
    """Return the exact runtime file map from the current build inputs."""
    binary = binary or root / "target/release/serein"
    host = host or root / "target/release/serein-host"
    model_source = model_source or root / "models/pack"
    license_source = license_source or root / "release/dependency-licenses"
    third_party_licenses = third_party_licenses or root / "docs/THIRD_PARTY_LICENSES.json"

    files: dict[str, pathlib.Path] = {
        "serein": _require_regular_file(binary, "release CLI"),
        "serein-host": _require_regular_file(host, "release native host"),
        "THIRD_PARTY_LICENSES.json": _require_regular_file(
            third_party_licenses, "third-party license inventory"
        ),
    }

    model_files = _tree_files(model_source, "model pack")
    model_relatives = {path.relative_to(model_source).as_posix() for path in model_files}
    missing_model_files = sorted(set(MODEL_FILES) - model_relatives)
    if missing_model_files:
        raise SystemExit(
            "Model pack is missing required files: " + ", ".join(missing_model_files)
        )
    unexpected_model_files = sorted(
        model_relatives - set(MODEL_FILES) - MODEL_SOURCE_ONLY_FILES
    )
    if unexpected_model_files:
        raise SystemExit(
            "Model pack contains files outside the runtime allowlist: "
            + ", ".join(unexpected_model_files[:8])
        )
    for name in MODEL_FILES:
        files[f"model/{name}"] = model_source / name

    license_files = _tree_files(license_source, "dependency license inventory")
    if not license_files:
        raise SystemExit(f"Dependency license inventory is empty: {license_source}")
    for source in license_files:
        inventory_relative = source.relative_to(license_source).as_posix()
        path_parts = pathlib.PurePosixPath(inventory_relative).parts
        word_parts = {
            word
            for component in path_parts
            for word in re.split(r"[-_.]+", component.casefold())
        }
        if (
            not LEGAL_NOTICE_NAME.fullmatch(source.name)
            or source.suffix.casefold() in UNSAFE_NOTICE_SUFFIXES
            or any(part.startswith(".") for part in path_parts)
            or word_parts & UNSAFE_PATH_PARTS
        ):
            raise SystemExit(
                "Unexpected non-license, hidden, temporary, or credential-like file "
                f"in dependency license inventory: {inventory_relative}"
            )
        relative = f"dependency-licenses/{inventory_relative}"
        files[relative] = source

    if "SHA256SUMS" in files:
        raise SystemExit("A runtime input conflicts with the generated checksum manifest.")
    return dict(sorted(files.items()))


def _check_replaceable_runtime(runtime: pathlib.Path, expected_files: set[str]) -> None:
    if runtime.is_symlink():
        raise SystemExit(f"Refusing to replace runtime symlink: {runtime}")
    if not runtime.exists():
        return
    if not runtime.is_dir():
        raise SystemExit(f"Refusing to replace non-directory runtime path: {runtime}")

    expected_directories: set[str] = set()
    for relative in expected_files:
        parent = pathlib.PurePosixPath(relative).parent
        while parent != pathlib.PurePosixPath("."):
            expected_directories.add(parent.as_posix())
            parent = parent.parent

    unexpected = []
    actual_file_paths: set[str] = set()
    for path in runtime.rglob("*"):
        relative = path.relative_to(runtime).as_posix()
        if path.is_symlink():
            unexpected.append(relative)
        elif path.is_dir():
            if relative not in expected_directories:
                unexpected.append(relative + "/")
        elif path.is_file():
            if relative != "SHA256SUMS":
                actual_file_paths.add(relative)
            if relative != "SHA256SUMS" and relative not in expected_files:
                unexpected.append(relative)
        else:
            unexpected.append(relative)
    if unexpected:
        rendered = ", ".join(sorted(unexpected)[:8])
        suffix = " …" if len(unexpected) > 8 else ""
        raise SystemExit(
            f"Runtime contains unexpected files or directories; preserving them and "
            f"refusing to replace {runtime}: {rendered}{suffix}"
        )

    checksum_file = runtime / "SHA256SUMS"
    if checksum_file.is_symlink() or not checksum_file.is_file():
        raise SystemExit(
            f"Runtime has no generated checksum manifest; preserving it and refusing to replace {runtime}."
        )
    previous_checksums: dict[str, str] = {}
    for line in checksum_file.read_text(encoding="utf-8").splitlines():
        digest_value, separator, relative = line.partition("  ")
        path = pathlib.PurePosixPath(relative)
        if (
            not separator
            or not re.fullmatch(r"[0-9a-f]{64}", digest_value)
            or not relative
            or "\\" in relative
            or path.is_absolute()
            or ".." in path.parts
            or path.as_posix() != relative
            or relative == "SHA256SUMS"
            or relative in previous_checksums
        ):
            raise SystemExit(
                f"Runtime checksum manifest is invalid; preserving it and refusing to replace {runtime}."
            )
        previous_checksums[relative] = digest_value

    if not actual_file_paths.issubset(previous_checksums):
        raise SystemExit(
            f"Runtime contains files without generated checksums; preserving it and refusing to replace {runtime}."
        )
    if not set(previous_checksums).issubset(expected_files):
        raise SystemExit(
            f"Runtime checksum manifest contains stale paths; preserving it and refusing to replace {runtime}."
        )
    for relative in actual_file_paths:
        if digest(runtime / relative) != previous_checksums[relative]:
            raise SystemExit(
                f"Runtime file was modified; preserving it and refusing to replace {runtime}: {relative}"
            )


def bundle_runtime(
    runtime: pathlib.Path = RUNTIME,
    *,
    sources: dict[str, pathlib.Path] | None = None,
) -> int:
    """Build a clean staged runtime and swap it in with rollback on failure."""
    sources = runtime_source_files() if sources is None else sources
    if not sources:
        raise SystemExit("Refusing to create an empty skill runtime.")
    for relative in sources:
        path = pathlib.PurePosixPath(relative)
        if (
            not relative
            or "\\" in relative
            or path.is_absolute()
            or ".." in path.parts
            or path.as_posix() != relative
            or relative == "SHA256SUMS"
        ):
            raise SystemExit(f"Invalid runtime source path: {relative!r}")
    runtime = pathlib.Path(runtime)
    parent = runtime.parent
    parent.mkdir(parents=True, exist_ok=True)
    assert_no_runtime_workdirs(runtime)
    _check_replaceable_runtime(runtime, set(sources))

    stage: pathlib.Path | None = pathlib.Path(
        tempfile.mkdtemp(prefix=f".{runtime.name}.stage-", dir=parent)
    )
    backup: pathlib.Path | None = None
    try:
        for relative, source in sources.items():
            destination = stage / pathlib.PurePosixPath(relative)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)

        checksum_lines = "".join(
            f"{digest(stage / pathlib.PurePosixPath(relative))}  {relative}\n"
            for relative in sorted(sources)
        )
        (stage / "SHA256SUMS").write_text(checksum_lines, encoding="utf-8")
        stage.chmod(0o755)

        if runtime.exists():
            backup = parent / f".{runtime.name}.backup-{uuid.uuid4().hex}"
            if backup.exists() or backup.is_symlink():
                raise SystemExit(f"Refusing to replace existing runtime backup path: {backup}")
            os.replace(runtime, backup)
        try:
            os.replace(stage, runtime)
            stage = None
        except BaseException:
            if backup is not None:
                try:
                    os.replace(backup, runtime)
                    backup = None
                except OSError as rollback_error:
                    raise RuntimeError(
                        f"Runtime replacement failed and rollback also failed; "
                        f"the previous runtime remains at {backup}."
                    ) from rollback_error
            raise

        if backup is not None:
            try:
                shutil.rmtree(backup)
                backup = None
            except OSError as cleanup_error:
                raise RuntimeError(
                    f"New runtime installed, but the previous generated runtime "
                    f"remains at {backup}; inspect it before removing it."
                ) from cleanup_error
        return len(sources)
    finally:
        if stage is not None and stage.exists():
            shutil.rmtree(stage)


def main() -> None:
    if (platform.system(), platform.machine().lower()) not in {
        ("Darwin", "arm64"), ("Darwin", "aarch64")
    }:
        raise SystemExit("The bundled runtime build currently supports macOS Apple silicon only.")

    sources = runtime_source_files()
    binary = sources["serein"]
    built_version = json.loads(subprocess.check_output([binary, "--version"], timeout=5))[
        "version"
    ]
    if built_version != VERSION:
        raise SystemExit(f"CLI version {built_version} does not match workspace {VERSION}.")

    file_count = bundle_runtime(RUNTIME, sources=sources)
    print(f"Bundled Serein {VERSION} for macOS Apple silicon ({file_count} files).")


if __name__ == "__main__":
    main()
