#!/usr/bin/env python3
"""Build Serein release artifacts and scan their publication contents."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import zipfile
from datetime import datetime, timezone

try:
    from . import bundle_skill
except ImportError:  # Running as a script or loading this file directly.
    _bundle_spec = importlib.util.spec_from_file_location(
        "serein_bundle_skill", pathlib.Path(__file__).with_name("bundle_skill.py")
    )
    if _bundle_spec is None or _bundle_spec.loader is None:
        raise ImportError("Unable to load tools/bundle_skill.py")
    bundle_skill = importlib.util.module_from_spec(_bundle_spec)
    sys.modules[_bundle_spec.name] = bundle_skill
    _bundle_spec.loader.exec_module(bundle_skill)


ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / "release"
VERSION = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.MULTILINE).group(1)
if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", VERSION):
    raise SystemExit("Workspace version must be a simple semantic version.")
AUDIT_PATH = ROOT / "docs/publication-audit.json"

SOURCE_ALLOWLIST = [
    ".github/RELEASE_NOTES.md",
    ".github/workflows/ci.yml",
    ".gitignore",
    "README.md",
    "LICENSE",
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "crates",
    "contracts",
    "fixtures",
    "tools",
    "tests",
    "skills",
    "adapters",
    "assets",
    "docs",
    "apps/extension",
    "release/dependency-licenses",
    "release/extension-identity.json",
    "models/manifest.json",
    "models/pack",
]

EXCLUDED_PARTS = {
    ".git",
    ".hg",
    ".svn",
    ".toolchain",
    "node_modules",
    "target",
    ".output",
    ".wxt",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".cache",
    "coverage",
    "dist",
    "playwright-report",
    "test-results",
}
EXCLUDED_NAMES = {".DS_Store", ".AppleDouble", "Thumbs.db"}
INTERNAL_PATHS = {
    "AGENTS.md",
    "SEREIN_BUILD_SPEC.md",
    "docs/STATE.md",
    "docs/design/ux-concept.png",
}
EXCLUDED_SUFFIXES = {
    ".pyc",
    ".pyo",
    ".tmp",
    ".temp",
    ".log",
    ".sqlite",
    ".sqlite3",
    ".db",
    ".pem",
    ".key",
    ".swp",
    ".bak",
}
TEXT_SUFFIXES = {
    ".cjs",
    ".css",
    ".html",
    ".js",
    ".json",
    ".md",
    ".mjs",
    ".py",
    ".rs",
    ".sh",
    ".svg",
    ".toml",
    ".ts",
    ".tsx",
    ".txt",
    ".xml",
    ".yaml",
    ".yml",
}
SENSITIVE_TEXT_PATTERNS = {
    "private_key_marker": re.compile(
        rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----"
    ),
    "github_token": re.compile(rb"(?:gh[pousr]_|github_pat_)[A-Za-z0-9_]{20,}"),
    "openai_style_key": re.compile(rb"\bsk-[A-Za-z0-9_-]{24,}"),
    "aws_access_key": re.compile(rb"\bAKIA[0-9A-Z]{16}\b"),
    "user_home_path": re.compile(rb"/(?:Users/[^/\s]+|home/(?!\.)[^/\s]+)/"),
    # A bare `/tmp/` is a generic OS constant. Require a concrete directory
    # below it, or the per-user temporary directory shape used by macOS.
    "mac_temp_path": re.compile(
        rb"/(?:private/)?tmp/[^/\s\x00]{1,128}/|"
        rb"/(?:private/)?var/folders/[A-Za-z0-9._-]{1,64}/"
        rb"[A-Za-z0-9._-]{1,64}/T/"
    ),
    "homebrew_prefix": re.compile(rb"/opt/" + rb"homebrew/"),
    "windows_user_path": re.compile(rb"[A-Za-z]:\\Users\\[^\\\s]+\\"),
}
BINARY_PATH_PATTERNS = (
    "user_home_path",
    "windows_user_path",
    "mac_temp_path",
    "homebrew_prefix",
)


def excluded(relative: pathlib.PurePath) -> bool:
    parts = relative.parts
    name = relative.name
    path_text = relative.as_posix()
    if path_text in INTERNAL_PATHS or path_text.startswith("docs/decisions/"):
        return True
    if any(part in EXCLUDED_PARTS for part in parts):
        return True
    if name in EXCLUDED_NAMES or name == ".env" or name.startswith(".env."):
        return name != ".env.example"
    return pathlib.Path(name).suffix.lower() in EXCLUDED_SUFFIXES


def source_files() -> tuple[list[pathlib.Path], dict[str, int]]:
    included: list[pathlib.Path] = []
    skipped: dict[str, int] = {}
    for entry in SOURCE_ALLOWLIST:
        source = ROOT / entry
        if not source.exists():
            continue
        candidates = [source] if source.is_file() else sorted(source.rglob("*"))
        for candidate in candidates:
            relative = candidate.relative_to(ROOT)
            if excluded(relative) or candidate.is_symlink():
                reason = "generated_or_sensitive_name" if excluded(relative) else "symlink"
                skipped[reason] = skipped.get(reason, 0) + 1
                continue
            if candidate.is_file():
                included.append(candidate)
    return sorted(set(included)), skipped


def scan_file(path: pathlib.Path, display_name: str) -> list[dict[str, str]]:
    binary = path.name in {"serein", "serein-host"} and "runtime" in path.parts
    if path.suffix.lower() not in TEXT_SUFFIXES and not binary:
        return []
    data = path.read_bytes()
    patterns = BINARY_PATH_PATTERNS if binary else SENSITIVE_TEXT_PATTERNS
    return [
        {"file": display_name, "rule": rule}
        for rule in patterns
        if SENSITIVE_TEXT_PATTERNS[rule].search(data)
    ]


def scan_zip(path: pathlib.Path) -> tuple[list[dict[str, str]], int, int]:
    findings: list[dict[str, str]] = []
    count = 0
    excluded_count = 0
    with zipfile.ZipFile(path) as archive:
        for member in archive.infolist():
            name = pathlib.PurePosixPath(member.filename)
            if excluded(name):
                excluded_count += 1
                findings.append({"file": f"{path.name}:{member.filename}", "rule": "excluded_member_present"})
                continue
            if member.is_dir():
                continue
            count += 1
            if name.suffix.lower() in TEXT_SUFFIXES:
                data = archive.read(member)
                findings.extend(
                    {"file": f"{path.name}:{member.filename}", "rule": rule}
                    for rule, pattern in SENSITIVE_TEXT_PATTERNS.items()
                    if pattern.search(data)
                )
            elif name.name in {"serein", "serein-host"} and "runtime" in name.parts:
                data = archive.read(member)
                for rule in BINARY_PATH_PATTERNS:
                    if SENSITIVE_TEXT_PATTERNS[rule].search(data):
                        findings.append({"file": f"{path.name}:{member.filename}", "rule": rule})
    return findings, count, excluded_count


def scan_tar(path: pathlib.Path) -> tuple[list[dict[str, str]], int, int]:
    findings: list[dict[str, str]] = []
    count = 0
    excluded_count = 0
    with tarfile.open(path, "r:gz") as archive:
        for member in archive.getmembers():
            name = pathlib.PurePosixPath(member.name)
            if excluded(name) or not member.isfile():
                if excluded(name):
                    excluded_count += 1
                    findings.append({"file": f"{path.name}:{member.name}", "rule": "excluded_member_present"})
                elif member.issym() or member.islnk():
                    findings.append({"file": f"{path.name}:{member.name}", "rule": "link_member_present"})
                continue
            count += 1
            if name.suffix.lower() in TEXT_SUFFIXES:
                stream = archive.extractfile(member)
                data = stream.read() if stream else b""
                findings.extend(
                    {"file": f"{path.name}:{member.name}", "rule": rule}
                    for rule, pattern in SENSITIVE_TEXT_PATTERNS.items()
                    if pattern.search(data)
                )
            elif name.name in {"serein", "serein-host"} and "runtime" in name.parts:
                # Rust binaries can retain dependency source paths even in release mode.
                stream = archive.extractfile(member)
                data = stream.read() if stream else b""
                for rule in BINARY_PATH_PATTERNS:
                    if SENSITIVE_TEXT_PATTERNS[rule].search(data):
                        findings.append({"file": f"{path.name}:{member.name}", "rule": rule})
    return findings, count, excluded_count


def add_tar_file(archive: tarfile.TarFile, source: pathlib.Path, archive_name: str) -> None:
    info = archive.gettarinfo(str(source), arcname=archive_name)
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    if info.isfile():
        with source.open("rb") as stream:
            archive.addfile(info, stream)
    elif info.isdir():
        archive.addfile(info)


def make_tar(destination: pathlib.Path, files: list[tuple[pathlib.Path, str]]) -> None:
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    with tarfile.open(temporary, "w:gz") as archive:
        for source, archive_name in files:
            add_tar_file(archive, source, archive_name)
    os.replace(temporary, destination)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def validate_skill_runtime(
    runtime: pathlib.Path | None = None,
    *,
    binary: pathlib.Path | None = None,
    host: pathlib.Path | None = None,
    model_source: pathlib.Path | None = None,
    license_source: pathlib.Path | None = None,
    third_party_licenses: pathlib.Path | None = None,
) -> None:
    runtime = pathlib.Path(runtime or ROOT / "skills/serein-context/runtime/macos-arm64")
    bundle_skill.assert_no_runtime_workdirs(runtime)
    sources = bundle_skill.runtime_source_files(
        root=ROOT,
        binary=binary,
        host=host,
        model_source=model_source,
        license_source=license_source,
        third_party_licenses=third_party_licenses,
    )
    if runtime.is_symlink() or not runtime.is_dir():
        raise SystemExit(f"Missing regular skill runtime directory: {runtime}")

    actual_files: set[str] = set()
    actual_directories: set[str] = set()
    for path in runtime.rglob("*"):
        relative = path.relative_to(runtime).as_posix()
        if path.is_symlink():
            raise SystemExit(f"Unexpected symlink in skill runtime: {relative}")
        if path.is_dir():
            actual_directories.add(relative)
        elif path.is_file():
            if relative != "SHA256SUMS":
                actual_files.add(relative)
        else:
            raise SystemExit(f"Unexpected non-file in skill runtime: {relative}")

    expected_files = set(sources)
    expected_directories: set[str] = set()
    for relative in expected_files:
        parent = pathlib.PurePosixPath(relative).parent
        while parent != pathlib.PurePosixPath("."):
            expected_directories.add(parent.as_posix())
            parent = parent.parent
    missing = sorted(expected_files - actual_files)
    unexpected = sorted(actual_files - expected_files)
    unexpected_directories = sorted(actual_directories - expected_directories)
    if missing or unexpected or unexpected_directories:
        details = []
        if missing:
            details.append("missing: " + ", ".join(missing[:8]))
        if unexpected:
            details.append("unexpected files: " + ", ".join(unexpected[:8]))
        if unexpected_directories:
            details.append("unexpected directories: " + ", ".join(unexpected_directories[:8]))
        raise SystemExit(
            "Skill runtime differs from the current build inputs ("
            + "; ".join(details)
            + "). Run tools/bundle_skill.py."
        )

    checksum_file = runtime / "SHA256SUMS"
    if checksum_file.is_symlink() or not checksum_file.is_file():
        raise SystemExit("Skill runtime checksum manifest is missing or not a regular file.")
    checksums: dict[str, str] = {}
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
            or relative in checksums
        ):
            raise SystemExit("Invalid skill runtime checksum manifest.")
        checksums[relative] = digest_value
    if set(checksums) != expected_files:
        raise SystemExit("Skill runtime checksum manifest differs from current build inputs.")

    for relative, source in sources.items():
        bundled_digest = sha256(runtime / relative)
        if bundled_digest != checksums[relative]:
            raise SystemExit(f"Skill runtime checksum mismatch: {relative}.")
        if bundled_digest != sha256(source):
            raise SystemExit(
                f"Skill runtime does not match its current build input: {relative}. "
                "Run tools/bundle_skill.py."
            )

    built = json.loads(subprocess.check_output([runtime / "serein", "--version"], timeout=5))
    if built.get("version") != VERSION:
        raise SystemExit("Bundled helper version differs from package version. Run tools/bundle_skill.py.")


def main() -> None:
    validate_skill_runtime()
    OUT.mkdir(exist_ok=True)
    generated: list[pathlib.Path] = []

    # Browser bundles are generated by WXT and checked for narrow permissions.
    for browser in ("chrome", "firefox"):
        source = ROOT / f"apps/extension/.output/{browser}-mv3"
        manifest_path = source / "manifest.json"
        if not manifest_path.is_file():
            raise SystemExit(f"Missing {manifest_path}; build both MV3 extensions first.")
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest.get("version") != VERSION:
            raise SystemExit(f"{browser} extension version differs from package version. Rebuild the extension.")
        expected = {"tabs", "storage", "alarms", "idle", "nativeMessaging"}
        if set(manifest.get("permissions", [])) != expected:
            raise SystemExit(f"Unexpected {browser} extension permission set.")
        if manifest.get("content_scripts") or manifest.get("host_permissions"):
            raise SystemExit(f"Unexpected broad page access in {browser} extension manifest.")
        repository = b"https://github.com/Maar10Herr/serein"
        if not any(repository in file.read_bytes() for file in source.rglob("*.js")):
            raise SystemExit(f"{browser} build is missing the public skill repository. Set SEREIN_SKILL_REPOSITORY and rebuild.")
        artifact = OUT / f"serein-{browser}-{VERSION}-unsigned.zip"
        with zipfile.ZipFile(artifact, "w", zipfile.ZIP_DEFLATED) as archive:
            for file in sorted(source.rglob("*")):
                if file.is_file() and not file.is_symlink() and not excluded(file.relative_to(source)):
                    archive.write(file, file.relative_to(source).as_posix())
            archive.write(ROOT / "LICENSE", "LICENSE")
            archive.write(ROOT / "README.md", "README.md")
        generated.append(artifact)

    skill_root = ROOT / "skills/serein-context"
    skill_artifact = OUT / f"serein-skills-{VERSION}.zip"
    with zipfile.ZipFile(skill_artifact, "w", zipfile.ZIP_DEFLATED) as archive:
        for file in sorted(skill_root.rglob("*")):
            if file.is_file() and not file.is_symlink() and not excluded(file.relative_to(skill_root)):
                archive.write(file, file.relative_to(ROOT / "skills").as_posix())
    generated.append(skill_artifact)

    # Build the scan report from an explicit source allowlist. Internal gate
    # notes and local build instructions are intentionally excluded.
    files, skipped = source_files()
    findings: list[dict[str, str]] = []
    for file in files:
        findings.extend(scan_file(file, file.relative_to(ROOT).as_posix()))
    source_count = len(files)
    archive_member_counts: dict[str, int] = {}
    archive_exclusions: dict[str, int] = {}
    for artifact in generated:
        if artifact.suffix == ".zip":
            found, count, excluded_count = scan_zip(artifact)
        else:
            found, count, excluded_count = scan_tar(artifact)
        findings.extend(found)
        archive_member_counts[artifact.name] = count
        archive_exclusions[artifact.name] = excluded_count

    finding_rules = {finding["rule"] for finding in findings}
    report = {
        "report_version": 1,
        "generated_at_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "scope": "Source archive allowlist, browser extension bundles, and bundled skill runtime.",
        "source_allowlist": SOURCE_ALLOWLIST,
        "source_file_count_scanned": source_count,
        "excluded_source_file_counts": skipped,
        "generated_archives": archive_member_counts,
        "excluded_archive_member_counts": archive_exclusions,
        "automated_checks": {
            "private_key_marker": "failed" if "private_key_marker" in finding_rules else "passed",
            "common_access_token_patterns": "failed" if finding_rules & {"github_token", "openai_style_key", "aws_access_key"} else "passed",
            "machine_specific_home_paths": "failed" if finding_rules & {"user_home_path", "windows_user_path", "homebrew_prefix"} else "passed",
            "temporary_paths": "failed" if "mac_temp_path" in finding_rules else "passed",
            "temporary_build_and_cache_members": "passed" if not any(archive_exclusions.values()) else "failed",
            "source_allowlist": "failed" if finding_rules & {"excluded_member_present", "link_member_present"} else "passed",
        },
        "findings": findings,
        "limitations": [
            "This scan covers packaged source files and generated archives; it does not inspect Git history.",
            "A clean automated scan is not a substitute for reviewing the files and release notes.",
            "Runtime binaries are checked for user, Homebrew, and concrete macOS temporary paths; a bare /tmp/ OS constant is not treated as a private path.",
            "The bundled helper currently supports macOS Apple silicon only.",
            "Unsigned artifacts have platform and validation limits described in the test report.",
        ],
    }
    AUDIT_PATH.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    findings.extend(scan_file(AUDIT_PATH, AUDIT_PATH.relative_to(ROOT).as_posix()))
    if findings:
        report["findings"] = findings
        for name in report["automated_checks"]:
            report["automated_checks"][name] = "failed"
        AUDIT_PATH.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        raise SystemExit(f"Publication scan found {len(findings)} issue(s); review {AUDIT_PATH}.")

    # Include this report in the source archive after writing it.
    files, skipped = source_files()
    source_artifact = OUT / f"serein-source-{VERSION}.tar.gz"
    make_tar(
        source_artifact,
        [(file, (pathlib.PurePosixPath("serein") / file.relative_to(ROOT)).as_posix()) for file in files],
    )
    generated.append(source_artifact)
    source_findings, source_archive_count, source_archive_excluded = scan_tar(source_artifact)
    if source_findings or source_archive_excluded:
        raise SystemExit(f"Source archive scan failed: {source_findings or source_archive_excluded}")
    archive_member_counts[source_artifact.name] = source_archive_count
    archive_exclusions[source_artifact.name] = source_archive_excluded
    report["generated_archives"] = archive_member_counts
    report["excluded_archive_member_counts"] = archive_exclusions
    AUDIT_PATH.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    # Repack once so the included audit report reflects the complete archive list.
    make_tar(
        source_artifact,
        [(file, (pathlib.PurePosixPath("serein") / file.relative_to(ROOT)).as_posix()) for file in source_files()[0]],
    )
    source_findings, source_archive_count, source_archive_excluded = scan_tar(source_artifact)
    if source_findings or source_archive_excluded:
        raise SystemExit(f"Final source archive scan failed: {source_findings or source_archive_excluded}")

    checksums = {artifact.name: sha256(artifact) for artifact in generated}
    (OUT / "SHA256SUMS").write_text(
        "".join(f"{digest}  {name}\n" for name, digest in sorted(checksums.items())),
        encoding="utf-8",
    )
    status = {
        "version": VERSION,
        "distribution": "unsigned-macos-arm64",
        "signed": False,
        "published": False,
        "publisher_key": None,
        "repository_owner": None,
        "artifacts": checksums,
        "release_accepted": False,
        "external_gates": [
            "publisher identity and signing",
            "Mozilla permanent installation signing",
            "six real local assistant executions",
            "cross-platform clean-machine matrix",
            "20 paired answer comparisons",
        ],
    }
    (OUT / "release-status.json").write_text(json.dumps(status, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"packaged": list(checksums), "signed": False, "published": False, "publication_scan": str(AUDIT_PATH.relative_to(ROOT))}, indent=2))


if __name__ == "__main__":
    main()
