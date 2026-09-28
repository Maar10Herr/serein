# Contract notes

These files are closed JSON Schema draft 2020-12 descriptions of the selected wire shapes. They are reviewable contracts, not generated Rust or TypeScript bindings; the Rust validators remain authoritative for runtime behavior. Run `tools/check_contracts.py` to validate the checked-in synthetic examples.

Known differences and limits:

- `Event.search_query` is required as a string or `null` because the TypeScript `Observation` requires the field and the extension sends it explicitly. Serde deserialization of Rust `Option<String>` also accepts the field being omitted.
- Rust retains `Envelope.payload` as a JSON value, with per-operation closed-object validation in `Envelope.validate` and deserialization in the host dispatcher.
- JSON Schema cannot express all runtime checks here: UUID parser behavior, the event title-plus-query 2048 UTF-8-byte limit, the ingest 65536-byte batch limit, the 90-day event-time window, per-site epoch freshness, monotonic policy epochs, or the combined 500-site policy cap. Runtime also applies capture filters and site policy checks beyond wire-shape validation. The schema's DNS label-length check is stricter than `valid_site`, which does not impose the DNS 63-character per-label limit.
- Canonical UUID `format` validation follows the IDs emitted by the extension. Rust's `uuid::parse_str` accepts additional UUID spellings. `client` and `label` Rust limits count UTF-8 bytes, while JSON Schema `maxLength` counts characters. Registry schemas encode the setup-time browser and adapter choices, while `read_registry` itself only checks that the registry version is supported and the JSON fields deserialize.
- Model-manifest schemas validate field shapes and bounds. The native loader additionally verifies asset sizes and hashes, tokenizer decoding, and token IDs while opening a model pack.
- `Recall` describes the CLI request. Recall results are assembled dynamically in `storage.rs`; there is no corresponding Rust response struct or TypeScript response type to use as a stable schema source.
- `dashboard-response.schema.json` specifies the research presentation fields returned by the native `dashboard` operation. The response also includes status fields assembled by the host, so the schema intentionally leaves the top level open while closing cards, corrections, and memory groups. `apps/extension/lib/types.ts` mirrors this response shape for the UI; runtime validation remains in Rust.
