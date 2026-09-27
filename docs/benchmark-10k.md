# Serein 0.1.2 · 10,000 atom retrieval benchmark

The process level benchmark exercises the release build with a full 10,000 atom vault, the storage cap defined by the build spec. It measures assistant recall after every atom has a current model vector and topic assignment.

## Result

| Measurement | Result |
| --- | ---: |
| Atoms ingested and indexed | 10,000 / 10,000 |
| Warm hybrid recall, 30 calls | median 242.26 ms; p95 247.34 ms; maximum 252.11 ms |
| Same vault in lexical fallback, 10 calls | median 8.85 ms; p95 9.06 ms; maximum 9.06 ms |
| Completed-index refresh, 10 calls | median 140.84 ms; p95 144.60 ms; maximum 144.60 ms |
| Recall time budget | 1,500 ms |
| Hybrid recall status | `ok` for all 30 calls |
| Context returned | median 6 records |
| Initial ingest | 25.94 s |
| Full vector and topic indexing | 42.90 s across 40 bounded refresh calls |

This run used release binaries on a Darwin arm64 Apple silicon host. It is not the dual-core, 4 GiB reference laptop. Recall samples include starting a fresh CLI process, opening the vault, and producing the response. The local file cache was warm after indexing and one discarded warm-up recall; no cold-cache measurement was taken.

The completed-index refresh is a proxy for process, database, and model initialization cost: it starts a new process and opens the installed model but indexes no atoms. Its median was about 141 ms. The lexical comparison used the same synthetic vault while its copied model manifest was temporarily hidden, then restored before cleanup. These numbers suggest model initialization contributes substantially to hybrid recall time, but do not isolate individual functions. The 1,500 ms recall budget was met in this environment; this result does not establish the target on lower-spec hardware.

## Method

Run from the repository root after building the release binaries:

```sh
cargo build --release
python3 tests/benchmark_retrieval_10k.py
```

The script creates a temporary, isolated Serein data directory and uses only generated event metadata. It ingests 10,000 unique search observations in batches of 32, spread evenly across ten shopping and research topics and twenty synthetic example domains. The measured query matches 1,000 of those atoms. It verifies the exact atom count, indexes until `pending_atoms` reaches zero, and confirms each recall returns evidence within the response byte limit. The benchmark never reads a browser profile or visits websites, and the temporary vault is removed when the run ends.

The p95 uses the nearest-rank definition. This is a process-level capacity and latency check, not an evaluation of relevance quality or a browser integration test.
