#!/usr/bin/env python3
"""Build Serein's inert, row-quantized pack from a pinned upstream snapshot.

This is a developer/release tool. It does not run during install or inference.
It intentionally reads only Hugging Face safetensors and tokenizer JSON; it
never imports pickle-based model formats or executes model repository code.

The converter needs NumPy and Hugging Face `tokenizers` (Rust-backed). For
example, use a project-local environment and install `numpy` and
`tokenizers==0.22.2`, which matches the Rust runtime's tokenizers crate.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
import struct
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Iterable

MODEL_ID = "sentence-transformers/static-similarity-mrl-multilingual-v1"
UPSTREAM_COMMIT = "bae9c8b1d48e8962a2ce7cb207662ed2a8441ccc"
UPSTREAM_BASE = f"https://huggingface.co/{MODEL_ID}/resolve/{UPSTREAM_COMMIT}"
MODEL_FILE = "0_StaticEmbedding/model.safetensors"
TOKENIZER_FILE = "0_StaticEmbedding/tokenizer.json"
MODEL_SHA256 = "8245ab78ee71dded845a82d2270fcb9e785b29dad0e1619f69d5390c47d9ba00"
TOKENIZER_SHA256 = "11aaf894a4ccf3d95e8830e27c0f8152791fbbff2b988e29a265580b86edd216"
SOURCE_ROWS = 105_879
SOURCE_DIMENSIONS = 1_024
DIMENSIONS = 256
MAX_SEQUENCE_TOKENS = 1_024  # Must stay aligned with crates/serein-core/src/model.rs.
MODEL_HASH = f"{MODEL_ID}@{UPSTREAM_COMMIT}:dim256:int8sym-v1"
EXPECTED_MODULES = [
    {
        "idx": 0,
        "name": "0",
        "path": "0_StaticEmbedding",
        "type": "sentence_transformers.models.StaticEmbedding",
    }
]
EXPECTED_ST_VERSION = "3.3.0.dev0"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"Could not read valid UTF-8 JSON from {path}: {exc}") from exc


def download(url: str, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "serein-model-pack-builder/1.0"},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response, destination.open("wb") as out:
            reported_commit = response.headers.get("X-Repo-Commit")
            if reported_commit and reported_commit != UPSTREAM_COMMIT:
                raise ValueError(
                    f"Hub returned commit {reported_commit}, expected pinned {UPSTREAM_COMMIT}"
                )
            shutil.copyfileobj(response, out, length=1024 * 1024)
    except (OSError, urllib.error.URLError) as exc:
        destination.unlink(missing_ok=True)
        raise RuntimeError(f"Failed to fetch pinned upstream asset {url}: {exc}") from exc


def fetch_snapshot(directory: Path) -> None:
    """Fetch only inert files needed to identify and convert the pinned model."""
    files = (
        MODEL_FILE,
        TOKENIZER_FILE,
        "modules.json",
        "config_sentence_transformers.json",
        "README.md",
    )
    for relative in files:
        path = directory / relative
        if not path.exists():
            print(f"Fetching {relative}", file=sys.stderr)
            download(f"{UPSTREAM_BASE}/{relative}", path)
    verify_source(directory)


def verify_source(directory: Path) -> tuple[Path, Path, dict[str, Any], dict[str, Any]]:
    model_path = directory / MODEL_FILE
    tokenizer_path = directory / TOKENIZER_FILE
    required = [
        model_path,
        tokenizer_path,
        directory / "modules.json",
        directory / "config_sentence_transformers.json",
        directory / "README.md",
    ]
    missing = [str(path) for path in required if not path.is_file()]
    if missing:
        raise ValueError("Pinned source snapshot is missing: " + ", ".join(missing))

    actual_model_hash = sha256(model_path)
    if actual_model_hash != MODEL_SHA256:
        raise ValueError(
            f"Safetensors SHA256 mismatch: got {actual_model_hash}, expected {MODEL_SHA256}"
        )
    actual_tokenizer_hash = sha256(tokenizer_path)
    if actual_tokenizer_hash != TOKENIZER_SHA256:
        raise ValueError(
            f"Tokenizer SHA256 mismatch: got {actual_tokenizer_hash}, expected {TOKENIZER_SHA256}"
        )

    modules = read_json(directory / "modules.json")
    if modules != EXPECTED_MODULES:
        raise ValueError(f"Unexpected upstream architecture modules: {modules!r}")
    config = read_json(directory / "config_sentence_transformers.json")
    versions = config.get("__version__", {})
    if versions.get("sentence_transformers") != EXPECTED_ST_VERSION:
        raise ValueError(
            "Pinned model's Sentence Transformers reference version changed: "
            f"{versions.get('sentence_transformers')!r}"
        )
    card = (directory / "README.md").read_text(encoding="utf-8")
    for required_card_text in (
        "license: apache-2.0",
        "EmbeddingBag(105879, 1024, mode='mean')",
    ):
        if required_card_text.casefold() not in card.casefold():
            raise ValueError(f"Pinned upstream model card omitted expected declaration {required_card_text!r}")

    tokenizer_json = read_json(tokenizer_path)
    validate_tokenizer_config(tokenizer_json)
    return model_path, tokenizer_path, tokenizer_json, config


def validate_tokenizer_config(tokenizer: dict[str, Any]) -> None:
    model = tokenizer.get("model", {})
    vocab = model.get("vocab", {})
    expected = {
        "[PAD]": 0,
        "[UNK]": 100,
        "[CLS]": 101,
        "[SEP]": 102,
        "[MASK]": 103,
    }
    if model.get("type") != "WordPiece" or len(vocab) != SOURCE_ROWS:
        raise ValueError("Tokenizer model/vocabulary does not match the pinned 105,879-row WordPiece table")
    if {token: vocab.get(token) for token in expected} != expected:
        raise ValueError("Pinned tokenizer's special-token IDs changed")
    if tokenizer.get("normalizer") != {
        "type": "BertNormalizer",
        "clean_text": True,
        "handle_chinese_chars": True,
        "strip_accents": None,
        "lowercase": True,
    }:
        raise ValueError("Pinned tokenizer's BERT normalization settings changed")
    if tokenizer.get("pre_tokenizer") != {"type": "BertPreTokenizer"}:
        raise ValueError("Pinned tokenizer's pre-tokenization settings changed")
    if tokenizer.get("post_processor", {}).get("type") != "TemplateProcessing":
        raise ValueError("Pinned tokenizer no longer declares its BERT template post-processor")


def open_safetensors(path: Path):
    """Return a read-only NumPy view after validating the safetensors header."""
    try:
        import numpy as np
    except ImportError as exc:  # pragma: no cover - environment-specific
        raise RuntimeError("NumPy is required; install it in a project-local developer environment") from exc

    size = path.stat().st_size
    with path.open("rb") as source:
        prefix = source.read(8)
        if len(prefix) != 8:
            raise ValueError("Truncated safetensors header")
        header_length = struct.unpack("<Q", prefix)[0]
        if not 2 <= header_length <= 16 * 1024 * 1024 or 8 + header_length > size:
            raise ValueError(f"Invalid safetensors header length {header_length}")
        header_bytes = source.read(header_length)
    try:
        header = json.loads(header_bytes)
    except json.JSONDecodeError as exc:
        raise ValueError(f"Invalid safetensors JSON header: {exc}") from exc

    if set(header) != {"embedding.weight"}:
        raise ValueError(f"Unexpected tensors in model safetensors: {sorted(header)}")
    tensor = header["embedding.weight"]
    if tensor.get("dtype") != "F32" or tensor.get("shape") != [SOURCE_ROWS, SOURCE_DIMENSIONS]:
        raise ValueError(f"Unexpected upstream tensor descriptor: {tensor!r}")
    start, end = tensor.get("data_offsets", [None, None])
    expected_bytes = SOURCE_ROWS * SOURCE_DIMENSIONS * 4
    data_start = 8 + header_length
    if start != 0 or end != expected_bytes or data_start + end != size:
        raise ValueError("Safetensors tensor byte range does not match its declared shape and file size")
    return np.memmap(path, mode="r", dtype="<f4", offset=data_start, shape=(SOURCE_ROWS, SOURCE_DIMENSIONS))


def synthetic_texts() -> list[str]:
    """Deterministic short synthetic strings, spread across 12 languages."""
    templates = [
        "Project {n}: Find a durable {topic} with quiet operation.",
        "Projekt {n}: Suche einen langlebigen {topic} mit leisem Betrieb.",
        "Project {n}: Zoek een stevige {topic} die stil werkt.",
        "Projet {n} : Trouver un {topic} solide et silencieux.",
        "Proyecto {n}: Busca un {topic} resistente y silencioso.",
        "Progetto {n}: Cerca un {topic} durevole e silenzioso.",
        "Projeto {n}: Encontre um {topic} durável e silencioso.",
        "Проект {n}: Найти надёжный {topic} с тихой работой.",
        "項目 {n}：静かに動く丈夫な {topic} を探す。",
        "项目 {n}：寻找耐用、运行安静的 {topic}。",
        "مشروع {n}: ابحث عن {topic} متين يعمل بهدوء.",
        "Dự án {n}: Tìm {topic} bền và hoạt động êm.",
    ]
    topics = [
        "desk lamp", "travel adapter", "bike lock", "ceramic mug", "backpack",
        "mechanical keyboard", "office chair", "air purifier", "reading light", "water bottle",
        "lampe de bureau", "Reiseadapter", "fietsslot", "cafétière", "cámara compacta",
        "tastiera meccanica", "mochila", "велосипедный замок", "空気清浄機", "保温ボトル",
    ]
    return [
        template.format(n=i, topic=topics[(i * 7 + language_index * 3) % len(topics)])
        for language_index, template in enumerate(templates)
        for i in range(20)
    ]


def ranking_cases() -> list[tuple[str, list[str]]]:
    return [
        (
            "Looking for a quiet desk lamp for evening reading.",
            [
                "Ich suche eine leise Schreibtischlampe zum Lesen am Abend.",
                "The bicycle needs a new rear tire before the trip.",
                "I need an adjustable reading light for late evenings.",
            ],
        ),
        (
            "I do not want a keyboard with loud switches.",
            [
                "I want a keyboard with quiet switches.",
                "I do not want a loud keyboard.",
                "The monitor stand has adjustable height.",
            ],
        ),
        (
            "Part number ZX-41B fits device ACME-RX7.",
            [
                "Part number ZX-41B fits device ACME-RX7.",
                "Part number ZX-41D fits device ACME-RX7.",
                "A compact camera is useful for travel.",
            ],
        ),
        (
            "I prefer coffee without sugar.",
            [
                "I prefer coffee with no sugar.",
                "I prefer coffee with sugar.",
                "The new chair has a soft seat.",
            ],
        ),
        (
            "The Dutch title is ‘Stille werkplek’.",
            [
                "The quiet workspace.",
                "De stille werkplek.",
                "La batterie doit durer toute la journée.",
            ],
        ),
        (
            "C++20 and RFC-9110 are exact identifiers.",
            [
                "The exact identifiers are C++20 and RFC-9110.",
                "The exact identifiers are C++17 and RFC-9111.",
                "Une lampe de lecture éclaire le bureau.",
            ],
        ),
    ]


def parity_texts() -> list[str]:
    return [
        "A quiet desk lamp helps me read after work.",
        "Literal [CLS] is model text when it appears in a title.",
        "Ich suche eine leise Schreibtischlampe zum Lesen.",
        "Ich möchte keine laute mechanische Tastatur.",
        "Ik zoek een stille bureaulamp om 's avonds te lezen.",
        "Onderdeel ZX-41B past op apparaat ACME-RX7.",
        "Je cherche une lampe de bureau silencieuse pour lire.",
        "Je ne veux pas d'un clavier mécanique bruyant.",
        "静かなデスクライトで夜に読書したいです。",
        "部品 ZX-41B は ACME-RX7 に適合します。",
        "我想找一盏安静的台灯，晚上可以看书。",
        "我不想要带有响亮按键的机械键盘。",
    ]


def runtime_f32_vector(ids: list[int], packed_weights, packed_scales):
    """Mirror model.rs's ordered f32 row sum and L2 normalization."""
    import numpy as np

    vector = np.zeros(DIMENSIONS, dtype=np.float32)
    for token_id in ids[:MAX_SEQUENCE_TOKENS]:
        scale = np.float32(packed_scales[token_id])
        row = np.asarray(packed_weights[token_id], dtype=np.int8).astype(np.float32)
        vector += row * scale
    norm_squared = np.float32(0.0)
    for value in vector:
        norm_squared = np.float32(norm_squared + np.float32(value * value))
    norm = np.sqrt(norm_squared, dtype=np.float32)
    if not np.isfinite(norm) or norm < np.float32(1e-12):
        return None
    return (vector / norm).astype(np.float32)


def write_parity_fixture(stage: Path) -> dict[str, Any]:
    import numpy as np

    Tokenizer = load_tokenizers()
    tokenizer = Tokenizer.from_file(str(stage / "tokenizer.json"))
    packed_weights = np.memmap(
        stage / "weights.i8", mode="r", dtype=np.int8, shape=(SOURCE_ROWS, DIMENSIONS)
    )
    packed_scales = np.memmap(stage / "scales.f32", mode="r", dtype="<f4", shape=(SOURCE_ROWS,))
    examples = []
    for text in parity_texts():
        token_ids = tokenizer.encode(text, add_special_tokens=False).ids
        if len(token_ids) > MAX_SEQUENCE_TOKENS:
            raise ValueError(f"Parity fixture exceeds native token cap: {text!r}")
        vector = runtime_f32_vector(token_ids, packed_weights, packed_scales)
        if vector is None:
            raise ValueError(f"Parity fixture produced a zero vector: {text!r}")
        examples.append(
            {
                "text": text,
                "token_ids": token_ids,
                "expected_vector_f32": [float(value) for value in vector],
            }
        )
    fixture = {
        "format": 1,
        "model_hash": MODEL_HASH,
        "dimensions": DIMENSIONS,
        "pooling": "ordered f32 sum of dequantized rows for encode(add_special_tokens=false), then ordered f32 L2 normalization",
        "examples": examples,
    }
    (stage / "parity.json").write_text(
        json.dumps(fixture, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8"
    )
    return {
        "sha256": sha256(stage / "parity.json"),
        "bytes": (stage / "parity.json").stat().st_size,
        "examples": len(examples),
        "languages": ["en", "de", "nl", "fr", "ja", "zh"],
    }


def normalize(vector):
    import numpy as np

    norm = float(np.linalg.norm(vector.astype(np.float64)))
    if not math.isfinite(norm) or norm < 1e-12:
        return None
    return vector.astype(np.float64) / norm


def pooled(ids: list[int], weights, scales=None):
    import numpy as np

    if not ids:
        return None
    if any(token_id < 0 or token_id >= SOURCE_ROWS for token_id in ids):
        raise ValueError("Tokenizer emitted an ID outside the pinned embedding table")
    if len(ids) > MAX_SEQUENCE_TOKENS:
        raise ValueError(
            f"Synthetic/reference text yielded {len(ids)} token IDs; native encoder cap is {MAX_SEQUENCE_TOKENS}"
        )
    if scales is None:
        vectors = np.asarray(weights[ids, :DIMENSIONS], dtype=np.float32)
    else:
        vectors = np.asarray(weights[ids, :], dtype=np.int8).astype(np.float32)
        vectors *= np.asarray(scales[ids], dtype=np.float32)[:, None]
    # Upstream EmbeddingBag(mode="mean") averages every ID returned by
    # encode_batch(add_special_tokens=False). No token weights or filters apply.
    result = vectors.mean(axis=0, dtype=np.float32)
    return normalize(result)


def load_tokenizers():
    try:
        from tokenizers import Tokenizer
    except ImportError as exc:  # pragma: no cover - environment-specific
        raise RuntimeError(
            "The validator needs Hugging Face tokenizers==0.22.2 installed in a project-local environment"
        ) from exc
    return Tokenizer


def collect_validation(tokenizer_source: Path, tokenizer_pack: Path, original_weights, packed_weights: Path, packed_scales: Path) -> dict[str, Any]:
    import numpy as np

    Tokenizer = load_tokenizers()
    reference_tokenizer = Tokenizer.from_file(str(tokenizer_source))
    pack_tokenizer = Tokenizer.from_file(str(tokenizer_pack))
    # Match sentence-transformers.models.StaticEmbedding.tokenize in the
    # pinned v3.3.0 reference: encode_batch(add_special_tokens=False).
    texts = synthetic_texts()
    cases = ranking_cases()
    all_texts = texts + [text for query, options in cases for text in (query, *options)]
    if len(texts) < 200:
        raise AssertionError("The multilingual parity suite must contain at least 200 synthetic texts")

    packed_q = np.memmap(packed_weights, mode="r", dtype=np.int8, shape=(SOURCE_ROWS, DIMENSIONS))
    packed_s = np.memmap(packed_scales, mode="r", dtype="<f4", shape=(SOURCE_ROWS,))
    cosine_values: list[float] = []
    compared_ids = 0
    max_tokens = 0
    for text in all_texts:
        reference_ids = reference_tokenizer.encode(text, add_special_tokens=False).ids
        packed_ids = pack_tokenizer.encode(text, add_special_tokens=False).ids
        if reference_ids != packed_ids:
            raise ValueError(f"Token ID mismatch for fixture {text!r}: {reference_ids!r} != {packed_ids!r}")
        compared_ids += 1
        max_tokens = max(max_tokens, len(reference_ids))
        ref = pooled(reference_ids, original_weights)
        quant = pooled(packed_ids, packed_q, packed_s)
        if (ref is None) != (quant is None):
            raise ValueError(f"Zero-vector behavior changed for fixture {text!r}")
        if ref is not None and quant is not None:
            similarity = float(np.dot(ref, quant))
            cosine_values.append(similarity)

    cosine = np.asarray(cosine_values, dtype=np.float64)
    minimum = float(cosine.min()) if cosine.size else None
    median = float(np.median(cosine)) if cosine.size else None
    thresholds = {"minimum": 0.98, "median": 0.995}
    if minimum is None or median is None or minimum < thresholds["minimum"] or median < thresholds["median"]:
        raise ValueError(
            f"Quantized normalized cosine misses release thresholds: min={minimum}, median={median}"
        )

    ranking_results = []
    top1_matches = 0
    ordering_preserved = 0
    pairwise_comparisons = 0
    for query, candidates in cases:
        query_ids = reference_tokenizer.encode(query, add_special_tokens=False).ids
        candidate_ids = [
            reference_tokenizer.encode(candidate, add_special_tokens=False).ids for candidate in candidates
        ]
        reference_query = pooled(query_ids, original_weights)
        packed_query = pooled(query_ids, packed_q, packed_s)
        if reference_query is None or packed_query is None:
            raise ValueError(f"Ranking query produced an empty vector: {query!r}")
        refs = [pooled(ids, original_weights) for ids in candidate_ids]
        quants = [pooled(ids, packed_q, packed_s) for ids in candidate_ids]
        if any(item is None for item in refs + quants):
            raise ValueError(f"Ranking candidate produced an empty vector for {query!r}")
        ref_scores = [float(np.dot(reference_query, item)) for item in refs]
        quant_scores = [float(np.dot(packed_query, item)) for item in quants]
        ref_order = sorted(range(len(ref_scores)), key=lambda idx: (-ref_scores[idx], idx))
        quant_order = sorted(range(len(quant_scores)), key=lambda idx: (-quant_scores[idx], idx))
        top1_matches += int(ref_order[0] == quant_order[0])
        for i in range(len(ref_scores)):
            for j in range(i + 1, len(ref_scores)):
                ref_delta = ref_scores[i] - ref_scores[j]
                quant_delta = quant_scores[i] - quant_scores[j]
                if abs(ref_delta) > 1e-7:
                    pairwise_comparisons += 1
                    ordering_preserved += int(ref_delta * quant_delta > 0)
        ranking_results.append(
            {
                "query": query,
                "reference_scores": [round(score, 8) for score in ref_scores],
                "quantized_scores": [round(score, 8) for score in quant_scores],
                "reference_order": ref_order,
                "quantized_order": quant_order,
            }
        )

    ranking_summary = {
        "cases": len(cases),
        "top1_order_matches": top1_matches,
        "pairwise_order_preserved": ordering_preserved,
        "pairwise_order_comparisons": pairwise_comparisons,
        "pairwise_order_agreement": (
            ordering_preserved / pairwise_comparisons if pairwise_comparisons else None
        ),
        "examples": ranking_results,
    }
    return {
        "synthetic_multilingual_texts": len(texts),
        "token_id_texts_compared": compared_ids,
        "token_id_exact_match": True,
        "maximum_token_count": max_tokens,
        "pooling_reference": "sentence-transformers v3.3.0 StaticEmbedding: mean over encode_batch(add_special_tokens=False) IDs",
        "quantized_normalized_cosine": {
            "cases": int(cosine.size),
            "minimum": minimum,
            "median": median,
            "thresholds": thresholds,
            "passed": True,
        },
        "held_out_neighbor_ranking": ranking_summary,
        "notes": [
            "Ranking compares quantized results to this pinned model's own cosine order; it is not a retrieval-quality claim.",
            "The six held-out cases include cross-lingual similarity, negation, and exact identifiers.",
        ],
    }


def convert(source: Path, output: Path, replace_existing: bool) -> dict[str, Any]:
    try:
        import numpy as np
    except ImportError as exc:  # pragma: no cover - environment-specific
        raise RuntimeError("NumPy is required; install it in a project-local developer environment") from exc

    model_path, tokenizer_path, tokenizer_json, config = verify_source(source)
    source_weights = open_safetensors(model_path)
    # A scan rejects NaN/Infinity before any byte is published. Chunking bounds
    # temporary memory and keeps the converter usable on ordinary developer CPUs.
    for start in range(0, SOURCE_ROWS, 8_192):
        block = source_weights[start : start + 8_192, :]
        if not np.isfinite(block).all():
            raise ValueError(f"Non-finite source embedding found in rows {start} onward")

    output = output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists() and not replace_existing:
        raise FileExistsError(f"Output already exists: {output}; pass --replace-existing to replace it")
    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.tmp-", dir=output.parent))
    try:
        weights_path = stage / "weights.i8"
        scales_path = stage / "scales.f32"
        scales = np.empty(SOURCE_ROWS, dtype="<f4")
        for start in range(0, SOURCE_ROWS, 8_192):
            end = min(start + 8_192, SOURCE_ROWS)
            source_block = np.asarray(source_weights[start:end, :DIMENSIONS], dtype=np.float32)
            max_abs = np.max(np.abs(source_block), axis=1)
            block_scales = (max_abs / np.float32(127.0)).astype("<f4")
            zero_rows = max_abs == 0.0
            block_scales[zero_rows] = np.float32(1.0)
            if not np.isfinite(block_scales).all() or np.any(block_scales <= 0):
                raise ValueError(f"Invalid quantization scale in rows {start}:{end}")
            scaled = source_block / block_scales[:, None]
            rounded = np.rint(scaled)
            if not np.isfinite(rounded).all() or np.any(rounded < -127) or np.any(rounded > 127):
                raise ValueError(f"Symmetric quantization would clip in rows {start}:{end}")
            quantized = rounded.astype(np.int8)
            if np.any(quantized[zero_rows] != 0):
                raise ValueError(f"Zero-vector rows did not remain zero in rows {start}:{end}")
            with weights_path.open("ab") as weights_file:
                weights_file.write(quantized.tobytes(order="C"))
            scales[start:end] = block_scales
        scales_path.write_bytes(scales.tobytes(order="C"))
        packed_tokenizer = stage / "tokenizer.json"
        shutil.copyfile(tokenizer_path, packed_tokenizer)
        repository_models = Path(__file__).resolve().parents[1] / "models"
        for extra in ("LICENSE-2.0.txt", "NOTICE.md"):
            source_extra = repository_models / extra
            if not source_extra.is_file():
                raise FileNotFoundError(f"Required model provenance asset is missing: {source_extra}")
            shutil.copyfile(source_extra, stage / extra)

        result = collect_validation(
            tokenizer_source=tokenizer_path,
            tokenizer_pack=packed_tokenizer,
            original_weights=source_weights,
            packed_weights=weights_path,
            packed_scales=scales_path,
        )
        parity = write_parity_fixture(stage)

        manifest = {
            "format": 1,
            "dimensions": DIMENSIONS,
            "rows": SOURCE_ROWS,
            "model_hash": MODEL_HASH,
            "weights_sha256": sha256(weights_path),
            "scales_sha256": sha256(scales_path),
            "tokenizer_sha256": sha256(packed_tokenizer),
            # StaticEmbedding calls encode_batch(add_special_tokens=False), so
            # no special token is injected and literal special-token text is
            # pooled exactly as upstream does.
            "special_ids": [],
        }
        (stage / "manifest.json").write_text(
            json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

        sizes = {name: (stage / name).stat().st_size for name in ("weights.i8", "scales.f32", "tokenizer.json")}
        if sizes["weights.i8"] != SOURCE_ROWS * DIMENSIONS:
            raise ValueError("Output weight table has an unexpected byte size")
        if sizes["scales.f32"] != SOURCE_ROWS * 4:
            raise ValueError("Output scale table has an unexpected byte size")

        model_dir = output.parent
        metadata = {
            "model": MODEL_ID,
            "upstream_commit": UPSTREAM_COMMIT,
            "license": "Apache-2.0",
            "source": {
                "embedding_file": MODEL_FILE,
                "embedding_sha256": MODEL_SHA256,
                "tokenizer_file": TOKENIZER_FILE,
                "tokenizer_sha256": TOKENIZER_SHA256,
                "sentence_transformers_version": config["__version__"]["sentence_transformers"],
            },
            "representation": {
                "rows": SOURCE_ROWS,
                "source_dimensions": SOURCE_DIMENSIONS,
                "dimensions": DIMENSIONS,
                "quantization": "symmetric row-wise int8; scale=max(abs(row))/127; round-to-nearest; zero rows use scale 1",
                "pooling": "mean over tokenizer IDs with add_special_tokens=false; no token weights",
                "special_ids": [],
                "model_hash": MODEL_HASH,
            },
            "pack": {
                "format": manifest["format"],
                "weights_sha256": manifest["weights_sha256"],
                "scales_sha256": manifest["scales_sha256"],
                "tokenizer_sha256": manifest["tokenizer_sha256"],
                "files_bytes": sizes,
                "parity_fixture_sha256": parity["sha256"],
            },
        }

        # Keep feature-space/license provenance outside the strict Rust
        # manifest, whose fields are deliberately owned by model.rs/schema.
        pack_manifest = json.loads((stage / "manifest.json").read_text(encoding="utf-8"))
        if set(pack_manifest) != {
            "format", "dimensions", "rows", "model_hash", "weights_sha256", "scales_sha256", "tokenizer_sha256", "special_ids"
        }:
            raise AssertionError("Runtime manifest must match the strict Rust contract")

        if output.exists():
            backup = output.with_name(output.name + ".old")
            if backup.exists():
                raise FileExistsError(f"Refusing to replace output while backup exists: {backup}")
            os.replace(output, backup)
            try:
                os.replace(stage, output)
            except Exception:
                os.replace(backup, output)
                raise
            shutil.rmtree(backup)
        else:
            os.replace(stage, output)
        (model_dir / "manifest.json").write_text(
            json.dumps(metadata, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        return {"runtime_manifest": manifest, "validation": result, "metadata": metadata}
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main(argv: Iterable[str] | None = None) -> int:
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path, help="already downloaded pinned snapshot (all required files)")
    parser.add_argument(
        "--source-cache",
        type=Path,
        default=root / "tools" / "cache" / "model-pack" / UPSTREAM_COMMIT,
        help="project-local cache for the pinned model downloads (default: tools/cache/model-pack/<commit>)",
    )
    parser.add_argument("--output", type=Path, default=root / "models" / "pack", help="pack output directory")
    parser.add_argument("--replace-existing", action="store_true", help="replace an existing pack after validation passes")
    parser.add_argument("--report", type=Path, default=root / "docs" / "model-pack-report.json", help="validation report JSON")
    args = parser.parse_args(argv)

    source = args.source_dir.resolve() if args.source_dir else args.source_cache.resolve()
    if not args.source_dir:
        fetch_snapshot(source)
    result = convert(source, args.output, args.replace_existing)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = {
        "status": "passed",
        "model": MODEL_ID,
        "upstream_commit": UPSTREAM_COMMIT,
        "upstream_license": "Apache-2.0",
        "source_embedding_sha256": MODEL_SHA256,
        "source_tokenizer_sha256": TOKENIZER_SHA256,
        "runtime_manifest": result["runtime_manifest"],
        "validation": result["validation"],
        "artifact_sizes_bytes": result["metadata"]["pack"]["files_bytes"],
        "runtime_parity_fixture": {
            "path": "models/pack/parity.json",
            **{
                "sha256": result["metadata"]["pack"]["parity_fixture_sha256"],
                "examples": 12,
                "languages": ["en", "de", "nl", "fr", "ja", "zh"],
            },
        },
        "reference_environment": {
            "python": sys.version.split()[0],
            "numpy": __import__("numpy").__version__,
            "tokenizers": __import__("tokenizers").__version__,
            "sentence_transformers_model_config": EXPECTED_ST_VERSION,
        },
        "method": {
            "tokenization": "Both source reference and prepared pack use the pinned tokenizer.json with add_special_tokens=false; IDs were compared per text.",
            "pooling": "Mean of original first-256 source rows for upstream reference; dequantized row sum/mean for the pack, followed by L2 normalization.",
            "ranking": "Held-out cosine ordering compared to the pinned source table, without interpreting it as retrieval-quality evidence.",
        },
        "provenance": {
            "model_card": f"https://huggingface.co/{MODEL_ID}/tree/{UPSTREAM_COMMIT}",
            "reference_pooling_source": "https://github.com/UKPLab/sentence-transformers/blob/v3.3.0/sentence_transformers/models/StaticEmbedding.py",
            "license_text": "models/LICENSE-2.0.txt",
            "upstream_license_declaration": f"https://huggingface.co/{MODEL_ID}/resolve/{UPSTREAM_COMMIT}/README.md",
        },
    }
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"status": "passed", "rows": SOURCE_ROWS, "dimensions": DIMENSIONS, "validation": result["validation"]["quantized_normalized_cosine"]}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (FileExistsError, RuntimeError, ValueError) as error:
        print(f"model pack build failed: {error}", file=sys.stderr)
        raise SystemExit(2)
