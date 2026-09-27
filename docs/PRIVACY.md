# Privacy model

Serein is designed to keep browsing context in the user's local account and make it available to a paired assistant only when the user invokes recall. This page describes the intended boundary; implementation, browser coverage, and executed test status are recorded in the [test report](TEST_REPORT.md). Read that report before treating a safeguard as verified.

## What capture can observe

The extension is limited to foreground tab metadata: a normalized title, the hostname, a search term extracted by an exact reviewed provider rule, and a coarse estimate of foreground time. It does not read page content, run content scripts, fetch pages, inspect browsing history, or backfill activity from before installation. It does not store raw URLs, arbitrary paths, fragments, or tracking parameters. Search terms in POST bodies and searches absent from both the title and URL are outside this design.

Capture is gated before metadata is serialized. Private windows, paused capture, excluded sites, unsupported URL schemes, local or intranet hosts, and sensitive metadata are dropped. The intended default filters cover authentication, private messaging, banking, adult, and medical portals, plus sensitive topic checks. A domain list and title checks cannot catch every sensitive visit; they are not anonymization. Users can pause capture, exclude a site, or choose selected-sites mode.

Titles and permitted search terms are untrusted input. Obvious emails, credentials, tokens, and phone-like identifiers are redacted; uncertain high-risk metadata is discarded. A foreground-time estimate is only an attention proxy. It cannot establish agreement, comprehension, ownership, or a lasting preference.

## Local storage and assistant disclosure

The design stores evidence and derived local representations in the user's Serein data directory. No analytics SDK is part of the design. Default diagnostics contain operational metadata such as error codes, durations, counts, and versions, not titles, queries, raw URLs, or full context packets. A debug export is opt-in and previews its contents before writing.

Recall returns a bounded context packet to the paired assistant integration on invocation, subject to the user's disclosure choice and the integration's configured scope. Once a packet has been sent to an external model or service, deleting the local copy cannot recall that disclosure or control the recipient's retention. Integrations should expose which evidence was returned and must not silently widen their filesystem or assistant access.

## Exclusion and deletion

Exclusion is an ordered protocol, not an immediate UI promise. The extension first persists the exclusion and advances its site epoch, clears queued and active state, then asks the host to delete matching evidence and invalidate dependent claims, topics, vectors, search indexes, and learned state. The UI must say “deletion pending” until the host commits. Stale queued events and old worker instances must not restore excluded data. Derived state is withheld until it can be rebuilt from surviving evidence.

SQLite deletion is logical removal, not guaranteed forensic erasure. Data may remain in SSD remapping, backups, crash dumps, or a conversation that already received recalled context. Full erase should remove the database, WAL/SHM files, and model-derived files, while explaining those remaining limits.
