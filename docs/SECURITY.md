# Security model

This page describes Serein's security boundaries. See the [test report](TEST_REPORT.md) for verified behavior and remaining limits. Serein has not undergone an independent security audit.

## Boundaries and untrusted input

The extension captures only the reviewed tab metadata described in [PRIVACY.md](PRIVACY.md). It communicates with a short-lived native host for local storage and recall. The host should accept only the versioned protocol contract, validate message sizes and fields, use parameterized SQL and safe filesystem operations, and exit after replying. Serein must not add a resident service, OS autostart entry, page scraper, content script, keylogger, or broader browser permission to fill gaps in observable metadata.

Page titles and search terms can contain hostile instructions, shell syntax, SQL fragments, HTML, path components, or log control characters. Treat them as inert text. Never put them into installer commands, skill instructions, paths, SQL syntax, HTML injection sinks, or format strings. Keep retrieved source text distinct from assistant instructions. This reduces injection risk; it cannot guarantee that a language model will always ignore hostile source text.

Assistant integrations receive only the bounded context returned for an explicit invocation and must follow their reviewed adapter contract. An adapter must not broaden access to the vault or global permissions. Compatibility is earned per integration: a listed or installed adapter is not evidence that it was exercised successfully.

## Storage, logs, and deletion

Use user-only directory and file permissions where the platform supports them and rely on normal OS account protections. The database is not encrypted at rest. Serein does not protect it from malware or another process running as the same user.

Default diagnostics are metadata-only. Raw URLs, titles, queries, and full context packets must stay out of crash reports and CI artifacts. Any opt-in debug export must show a preview before writing. Do not add analytics or telemetry.

Exclusion and erase must invalidate evidence and every dependent representation, reject events from old epochs, and complete only after the host commits. SQLite deletion cannot promise forensic erasure from SSDs, backups, crash dumps, or data already returned to an assistant. A full erase should also remove database sidecars and model-derived files and state this limitation clearly.
