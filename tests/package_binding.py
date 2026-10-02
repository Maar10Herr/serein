#!/usr/bin/env python3
"""Focused tests for runtime staging, exact artifact binding, and binary scans."""

from __future__ import annotations

import hashlib
import io
import json
import pathlib
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

TOOLS = pathlib.Path(__file__).resolve().parents[1] / "tools"
sys.path.insert(0, str(TOOLS))

import bundle_skill  # noqa: E402
import package as package_tool  # noqa: E402


class PackageBindingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="serein-package-binding-")
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.binary = self.root / "target/release/serein"
        self.host = self.root / "target/release/serein-host"
        self.model_source = self.root / "models/pack"
        self.license_source = self.root / "release/dependency-licenses"
        self.third_party_licenses = self.root / "docs/THIRD_PARTY_LICENSES.json"
        self.runtime = self.root / "runtime/macos-arm64"

        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(b"synthetic-cli-build-v1")
        self.host.write_bytes(b"synthetic-host-build-v1")
        self.model_source.mkdir(parents=True)
        for name in bundle_skill.MODEL_FILES:
            (self.model_source / name).write_bytes(f"model:{name}:v1".encode())
        (self.model_source / "parity.json").write_text("{}\n")
        self.license_source.mkdir(parents=True)
        (self.license_source / "example" / "LICENSE-MIT").parent.mkdir()
        (self.license_source / "example" / "LICENSE-MIT").write_text("synthetic license\n")
        self.third_party_licenses.parent.mkdir()
        self.third_party_licenses.write_text("{}\n")

    def sources(self) -> dict[str, pathlib.Path]:
        return bundle_skill.runtime_source_files(
            root=self.root,
            binary=self.binary,
            host=self.host,
            model_source=self.model_source,
            license_source=self.license_source,
            third_party_licenses=self.third_party_licenses,
        )

    def package_validate(self) -> None:
        with mock.patch.object(
            package_tool.subprocess,
            "check_output",
            return_value=json.dumps({"version": package_tool.VERSION}).encode(),
        ):
            package_tool.validate_skill_runtime(
                self.runtime,
                binary=self.binary,
                host=self.host,
                model_source=self.model_source,
                license_source=self.license_source,
                third_party_licenses=self.third_party_licenses,
            )

    def rewrite_manifest(self) -> None:
        files = sorted(
            path
            for path in self.runtime.rglob("*")
            if path.is_file() and path.name != "SHA256SUMS"
        )
        (self.runtime / "SHA256SUMS").write_text(
            "".join(
                f"{hashlib.sha256(path.read_bytes()).hexdigest()}  "
                f"{path.relative_to(self.runtime).as_posix()}\n"
                for path in files
            )
        )

    def test_bundle_stages_current_sources_and_preserves_unexpected_files(self) -> None:
        initial_count = bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        self.assertEqual(initial_count, len(self.sources()))
        self.assertEqual(
            {path.relative_to(self.runtime).as_posix() for path in self.runtime.rglob("*") if path.is_file()},
            set(self.sources()) | {"SHA256SUMS"},
        )
        self.assertNotIn("model/parity.json", self.sources())

        self.binary.write_bytes(b"synthetic-cli-build-v2")
        (self.model_source / "weights.i8").write_bytes(b"model weights v2")
        bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        self.assertEqual((self.runtime / "serein").read_bytes(), self.binary.read_bytes())
        self.assertEqual(
            (self.runtime / "model/weights.i8").read_bytes(),
            (self.model_source / "weights.i8").read_bytes(),
        )

        bundled_weights = (self.runtime / "model/weights.i8").read_bytes()
        (self.runtime / "model/weights.i8").write_bytes(b"user edit")
        with self.assertRaisesRegex(SystemExit, "Runtime file was modified; preserving it"):
            bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        self.assertEqual((self.runtime / "model/weights.i8").read_bytes(), b"user edit")
        (self.runtime / "model/weights.i8").write_bytes(bundled_weights)

        unexpected = self.runtime / "user-notes.txt"
        unexpected.write_text("keep this file\n")
        old_cli = (self.runtime / "serein").read_bytes()
        with self.assertRaisesRegex(SystemExit, "preserving them"):
            bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        self.assertEqual(unexpected.read_text(), "keep this file\n")
        self.assertEqual((self.runtime / "serein").read_bytes(), old_cli)

    def test_packager_rejects_self_checksummed_extra_and_stale_files(self) -> None:
        bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        self.package_validate()

        extra = self.runtime / "old-model.bin"
        extra.write_bytes(b"stale output")
        self.rewrite_manifest()
        with self.assertRaisesRegex(SystemExit, "unexpected files: old-model.bin"):
            self.package_validate()
        extra.unlink()

        cli = self.runtime / "serein"
        cli.write_bytes(b"old same-version executable")
        self.rewrite_manifest()
        with self.assertRaisesRegex(SystemExit, "does not match its current build input: serein"):
            self.package_validate()

    def test_packager_requires_scales_even_with_matching_manifest(self) -> None:
        bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        (self.runtime / "model/scales.f32").unlink()
        self.rewrite_manifest()
        with self.assertRaisesRegex(SystemExit, "missing: model/scales.f32"):
            self.package_validate()

    def test_model_pack_rejects_non_allowlisted_and_hidden_files(self) -> None:
        for name in ("modelpack.tmp", ".env"):
            with self.subTest(name=name):
                unexpected = self.model_source / name
                unexpected.write_text("not part of the runtime model allowlist\n")
                with self.assertRaisesRegex(SystemExit, "outside the runtime allowlist"):
                    self.sources()
                self.assertTrue(unexpected.is_file())
                unexpected.unlink()

    def test_license_inventory_rejects_hidden_temporary_and_credential_files(self) -> None:
        for relative in (".env", "LICENSE.tmp", "credentials/LICENSE-MIT"):
            with self.subTest(relative=relative):
                unexpected = self.license_source / relative
                unexpected.parent.mkdir(parents=True, exist_ok=True)
                unexpected.write_text("not a package license notice\n")
                with self.assertRaisesRegex(SystemExit, "Unexpected non-license"):
                    self.sources()
                self.assertTrue(unexpected.is_file())
                unexpected.unlink()
                parent = unexpected.parent
                while parent != self.license_source and not any(parent.iterdir()):
                    parent.rmdir()
                    parent = parent.parent

    def test_leftover_stage_or_backup_is_preserved_and_blocks_package(self) -> None:
        bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        for kind in ("stage", "backup"):
            with self.subTest(kind=kind):
                workdir = self.runtime.parent / f".{self.runtime.name}.{kind}-recoverable"
                workdir.mkdir()
                marker = workdir / "preserve-me.txt"
                marker.write_text("recoverable runtime workdir\n")
                with self.assertRaisesRegex(
                    SystemExit, "Incomplete skill runtime staging or backup"
                ):
                    self.package_validate()
                self.assertEqual(marker.read_text(), "recoverable runtime workdir\n")
                marker.unlink()
                workdir.rmdir()

    def test_failed_runtime_swap_restores_previous_generated_tree(self) -> None:
        bundle_skill.bundle_runtime(self.runtime, sources=self.sources())
        old_cli = (self.runtime / "serein").read_bytes()
        self.binary.write_bytes(b"replacement cli")
        real_replace = bundle_skill.os.replace

        def fail_stage_install(source: pathlib.Path, destination: pathlib.Path) -> None:
            if source.name.startswith(f".{self.runtime.name}.stage-") and destination == self.runtime:
                raise OSError("injected staged-install failure")
            real_replace(source, destination)

        with mock.patch.object(bundle_skill.os, "replace", side_effect=fail_stage_install):
            with self.assertRaisesRegex(OSError, "injected staged-install failure"):
                bundle_skill.bundle_runtime(self.runtime, sources=self.sources())

        self.assertEqual((self.runtime / "serein").read_bytes(), old_cli)
        self.assertTrue((self.runtime / "SHA256SUMS").is_file())
        self.assertFalse(list(self.runtime.parent.glob(f".{self.runtime.name}.backup-*")))
        self.assertFalse(list(self.runtime.parent.glob(f".{self.runtime.name}.stage-*")))

    def test_binary_path_scan_ignores_bare_tmp_constant_and_catches_concrete_paths(self) -> None:
        binary = self.runtime
        binary.mkdir(parents=True)
        (binary / "serein").write_bytes(b"OS constant: /tmp/\x00")
        findings = package_tool.scan_file(binary / "serein", "runtime/serein")
        self.assertNotIn("mac_temp_path", {finding["rule"] for finding in findings})

        payload = (
            b"/tmp/" + b"build-123/obj\x00"
            + b"/var/" + b"folders/ab/xy/T/build\x00"
            + b"/opt/" + b"homebrew/bin/tool\x00"
        )
        (binary / "serein").write_bytes(payload)
        findings = package_tool.scan_file(binary / "serein", "runtime/serein")
        self.assertEqual(
            {finding["rule"] for finding in findings},
            {"mac_temp_path", "homebrew_prefix"},
        )

        member_name = "skills/serein-context/runtime/macos-arm64/serein"
        archive_zip = self.root / "runtime.zip"
        with zipfile.ZipFile(archive_zip, "w") as archive:
            archive.writestr(member_name, payload)
        zipped_findings, file_count, excluded_count = package_tool.scan_zip(archive_zip)
        self.assertEqual(file_count, 1)
        self.assertEqual(excluded_count, 0)
        self.assertEqual(
            {finding["rule"] for finding in zipped_findings},
            {"mac_temp_path", "homebrew_prefix"},
        )

        archive_tar = self.root / "runtime.tar.gz"
        with tarfile.open(archive_tar, "w:gz") as archive:
            member = tarfile.TarInfo(name=f"serein/{member_name}")
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))
        tar_findings, file_count, excluded_count = package_tool.scan_tar(archive_tar)
        self.assertEqual(file_count, 1)
        self.assertEqual(excluded_count, 0)
        self.assertEqual(
            {finding["rule"] for finding in tar_findings},
            {"mac_temp_path", "homebrew_prefix"},
        )


if __name__ == "__main__":
    unittest.main()
