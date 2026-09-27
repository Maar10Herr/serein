# Serein v0.1.0-beta.2

Serein brings user-controlled browsing context to compatible AI assistants. The extension captures permitted foreground metadata after consent, and a local one-shot helper stores it in a SQLite vault. Recall is bounded, attributed to its sources, and initiated by the assistant when relevant.

This release includes:

- Chromium and Firefox MV3 extension packages with English, German, Dutch, Simplified Chinese, Japanese and Spanish interfaces, light/dark themes, capture controls, evidence review and site deletion.
- A macOS arm64 native runtime with the compact multilingual model and CLI.
- The `serein-context` skill, installable from this repository with the Skills CLI. The extension can request that installation during pairing.
- Source archives, license notices and SHA-256 checksums.

Serein's original code and skill are licensed under GPL-3.0-only. The bundled model and third-party dependencies retain their respective licenses; notices are included with the native runtime and source archive.

The browser packages are unsigned. Chromium can load the extracted build unpacked; Firefox can load its package temporarily through `about:debugging`. The native archive supports macOS arm64. See the README for setup and the test report for verified behavior and validation scope. Retrieval quality and end-to-end execution across all six assistants are outside the scope of this beta's validation.
