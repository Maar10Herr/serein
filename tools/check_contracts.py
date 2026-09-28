#!/usr/bin/env python3
"""Validate checked-in Serein JSON Schemas and synthetic wire examples.

Uses jsonschema when installed. A small deterministic fallback handles the
keywords used by these schemas, so the check remains useful in a stock Python
runtime without installing dependencies.
"""

from __future__ import annotations

import copy
import json
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
CONTRACTS = ROOT / "contracts"
SCHEMAS = {
    "event": "event.schema.json",
    "native": "native-envelope.schema.json",
    "recall": "recall-request.schema.json",
    "policy": "policy.schema.json",
    "registry": "registry.schema.json",
    "setup": "setup-ticket.schema.json",
    "manifest": "model-manifest.schema.json",
    "dashboard": "dashboard-response.schema.json",
}


class ContractError(Exception):
    pass


def resolve_pointer(document: dict[str, Any], ref: str) -> dict[str, Any]:
    if not ref.startswith("#/"):
        raise ContractError(f"unsupported non-local $ref: {ref}")
    value: Any = document
    for part in ref[2:].split("/"):
        part = part.replace("~1", "/").replace("~0", "~")
        value = value[part]
    return value


def is_type(value: Any, expected: str) -> bool:
    if expected == "object":
        return isinstance(value, dict)
    if expected == "array":
        return isinstance(value, list)
    if expected == "string":
        return isinstance(value, str)
    if expected == "boolean":
        return isinstance(value, bool)
    if expected == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if expected == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if expected == "null":
        return value is None
    return False


def check_subset(schema: dict[str, Any], value: Any, root: dict[str, Any], path: str = "$", depth: int = 0) -> None:
    if depth > 100:
        raise ContractError(f"schema recursion limit at {path}")
    if "$ref" in schema:
        check_subset(resolve_pointer(root, schema["$ref"]), value, root, path, depth + 1)

    types = schema.get("type")
    if types is not None:
        allowed_types = types if isinstance(types, list) else [types]
        if not any(is_type(value, t) for t in allowed_types):
            raise ContractError(f"{path}: expected type {allowed_types}")

    if "const" in schema and value != schema["const"]:
        raise ContractError(f"{path}: value differs from const")
    if "enum" in schema and value not in schema["enum"]:
        raise ContractError(f"{path}: value is outside enum")

    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            raise ContractError(f"{path}: string is too short")
        if len(value) > schema.get("maxLength", sys.maxsize):
            raise ContractError(f"{path}: string is too long")
        if "pattern" in schema and re.search(schema["pattern"], value) is None:
            raise ContractError(f"{path}: string does not match pattern")
        fmt = schema.get("format")
        if fmt == "uuid" and re.fullmatch(
            r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", value
        ) is None:
            raise ContractError(f"{path}: expected canonical UUID")
        if fmt == "date-time":
            if re.fullmatch(r"\d{4}-\d{2}-\d{2}T.+(?:Z|[+-]\d{2}:\d{2})", value) is None:
                raise ContractError(f"{path}: expected RFC 3339 date-time")
            try:
                datetime.fromisoformat(value.replace("Z", "+00:00"))
            except ValueError as exc:
                raise ContractError(f"{path}: expected date-time") from exc

    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if value < schema.get("minimum", float("-inf")):
            raise ContractError(f"{path}: number is below minimum")
        if value > schema.get("maximum", float("inf")):
            raise ContractError(f"{path}: number is above maximum")

    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            raise ContractError(f"{path}: array is too short")
        if len(value) > schema.get("maxItems", sys.maxsize):
            raise ContractError(f"{path}: array is too long")
        if "items" in schema:
            for index, item in enumerate(value):
                check_subset(schema["items"], item, root, f"{path}[{index}]", depth + 1)

    if isinstance(value, dict):
        missing = [key for key in schema.get("required", []) if key not in value]
        if missing:
            raise ContractError(f"{path}: missing required field(s): {', '.join(missing)}")
        properties = schema.get("properties", {})
        for key, child in value.items():
            if key in properties:
                check_subset(properties[key], child, root, f"{path}.{key}", depth + 1)
            elif schema.get("additionalProperties") is False:
                raise ContractError(f"{path}: unknown field {key!r}")
            elif isinstance(schema.get("additionalProperties"), dict):
                check_subset(schema["additionalProperties"], child, root, f"{path}.{key}", depth + 1)

    for sub in schema.get("allOf", []):
        check_subset(sub, value, root, path, depth + 1)
    if "anyOf" in schema:
        errors = []
        for sub in schema["anyOf"]:
            try:
                check_subset(sub, value, root, path, depth + 1)
                break
            except ContractError as exc:
                errors.append(str(exc))
        else:
            raise ContractError(f"{path}: no anyOf branch matched")
    if "not" in schema:
        try:
            check_subset(schema["not"], value, root, path, depth + 1)
        except ContractError:
            pass
        else:
            raise ContractError(f"{path}: not schema matched")
    if "if" in schema:
        try:
            check_subset(schema["if"], value, root, path, depth + 1)
        except ContractError:
            branch = schema.get("else")
        else:
            branch = schema.get("then")
        if branch is not None:
            check_subset(branch, value, root, path, depth + 1)


def examples() -> dict[str, Any]:
    event = {
        "event_id": "00000000-0000-4000-8000-000000000001",
        "visit_id": "00000000-0000-4000-8000-000000000002",
        "site_key": "research.example",
        "site_epoch": 3,
        "observed_at": "2026-09-27T00:00:00Z",
        "kind": "search",
        "title": "Mock shelf width 42 cm",
        "search_query": "mock shelf test width 42 cm",
        "foreground_seconds": 2,
    }
    policy = {
        "consent": True,
        "paused": False,
        "recall_enabled": True,
        "selected_only": False,
        "selected_sites": [],
        "excluded_sites": ["private.example"],
        "capture_epoch": 4,
    }
    recall = {
        "protocol": 1,
        "request_id": "00000000-0000-4000-8000-000000000003",
        "client": "synthetic-check",
        "vault": "default",
        "query": "What width is specified for the mock shelf test fixture?",
        "facets": ["mock shelf", "measurement"],
        "scope": ["research", "projects"],
        "max_bytes": 4096,
        "budget_ms": 1500,
    }
    native_base = {
        "protocol": 1,
        "request_id": "00000000-0000-4000-8000-000000000004",
        "source_id": "00000000-0000-4000-8000-000000000005",
        "capture_epoch": 4,
    }
    native = []
    payloads = {
        "hello": {"nonce": "a" * 64},
        "ingest": {"events": [event]},
        "status": {},
        "dashboard": {},
        "policy.update": policy,
        "forget": {"site": "private.example", "atom_id": None, "site_epoch": 5},
        "feedback": {"atom_id": "00000000-0000-4000-8000-000000000006", "action": "do_not_use", "text": None},
        "receipts": {},
    }
    for op, payload in payloads.items():
        native.append({**native_base, "op": op, "payload": payload})

    connection = {
        "source_id": "00000000-0000-4000-8000-000000000005",
        "vault_id": "00000000-0000-4000-8000-000000000007",
        "extension_id": "abcdefghijklmnopabcdefghijklmnop",
        "browser": "chrome",
        "nonce_hash": "a" * 64,
        "expires_at": 1790000000,
        "paired": True,
        "adapters": ["codex", "generic"],
        "label": "Synthetic local browser",
    }
    registry = {"version": 1, "default_vault": connection["vault_id"], "connections": [connection]}
    setup = {
        "protocol": 1,
        "source_id": "00000000-0000-4000-8000-000000000005",
        "extension_id": "abcdefghijklmnopabcdefghijklmnop",
        "browser": "chrome",
        "nonce": "a" * 64,
        "expires_at": 1790000000,
        "adapters": ["codex"],
        "label": "Synthetic local browser",
        "consent": True,
        "install_skills": True,
        "skill_repository": "https://github.com/Maar10Herr/serein",
    }
    manifest = {
        "format": 1,
        "dimensions": 256,
        "rows": 2,
        "model_hash": "synthetic-model-v1",
        "weights_sha256": "b" * 64,
        "scales_sha256": "c" * 64,
        "tokenizer_sha256": "d" * 64,
        "special_ids": [0, 1],
    }
    card = {
        "id": "00000000-0000-4000-8000-000000000008",
        "site": "research.example",
        "text": "Mock shelf width 42 cm",
        "state": "observed",
        "last_seen": "2026-09-27T00:00:00Z",
        "sessions": 2,
        "sites": 1,
        "kind": "visit",
        "prominent": True,
        "corrections": [],
    }
    dashboard = {
        "cards": [card],
        "memories": [{
            "id": "00000000-0000-4000-8000-000000000009",
            "label": "Mock shelf research",
            "last_seen": "2026-09-27T00:00:00Z",
            "sessions": 2,
            "sites": 2,
            "evidence_count": 2,
            "items": [card],
        }],
        "topics": [],
        "model_available": True,
        "index_mode": "hybrid",
    }
    return {"event": event, "policy": policy, "recall": recall, "native": native, "registry": registry, "setup": setup, "manifest": manifest, "dashboard": dashboard}


def unknown_mutations(examples_by_name: dict[str, Any]) -> list[tuple[str, dict[str, Any]]]:
    cases: list[tuple[str, dict[str, Any]]] = []
    for key in ("event", "policy", "recall", "registry", "setup", "manifest"):
        item = copy.deepcopy(examples_by_name[key])
        item["unexpected_field"] = True
        cases.append((key, item))

    top_level = copy.deepcopy(examples_by_name["native"][0])
    top_level["unexpected_field"] = True
    cases.append(("native", top_level))

    nested_event = copy.deepcopy(next(x for x in examples_by_name["native"] if x["op"] == "ingest"))
    nested_event["payload"]["events"][0]["unexpected_field"] = True
    cases.append(("native", nested_event))
    nested_policy = copy.deepcopy(next(x for x in examples_by_name["native"] if x["op"] == "policy.update"))
    nested_policy["payload"]["unexpected_field"] = True
    cases.append(("native", nested_policy))

    nested_batch = copy.deepcopy(next(x for x in examples_by_name["native"] if x["op"] == "ingest"))
    nested_batch["payload"]["unexpected_field"] = True
    cases.append(("native", nested_batch))
    nested_forget = copy.deepcopy(next(x for x in examples_by_name["native"] if x["op"] == "forget"))
    nested_forget["payload"]["unexpected_field"] = True
    cases.append(("native", nested_forget))
    nested_feedback = copy.deepcopy(next(x for x in examples_by_name["native"] if x["op"] == "feedback"))
    nested_feedback["payload"]["unexpected_field"] = True
    cases.append(("native", nested_feedback))

    nested_connection = copy.deepcopy(examples_by_name["registry"])
    nested_connection["connections"][0]["unexpected_field"] = True
    cases.append(("registry", nested_connection))
    nested_dashboard_card = copy.deepcopy(examples_by_name["dashboard"])
    nested_dashboard_card["cards"][0]["unexpected_field"] = True
    cases.append(("dashboard", nested_dashboard_card))
    nested_dashboard_memory = copy.deepcopy(examples_by_name["dashboard"])
    nested_dashboard_memory["memories"][0]["unexpected_field"] = True
    cases.append(("dashboard", nested_dashboard_memory))
    return cases


def main() -> int:
    schemas = {key: json.loads((CONTRACTS / name).read_text(encoding="utf-8")) for key, name in SCHEMAS.items()}
    examples_by_name = examples()
    cases: list[tuple[str, dict[str, Any]]] = [
        ("event", examples_by_name["event"]),
        ("policy", examples_by_name["policy"]),
        ("recall", examples_by_name["recall"]),
        ("registry", examples_by_name["registry"]),
        ("setup", examples_by_name["setup"]),
        ("manifest", examples_by_name["manifest"]),
        ("dashboard", examples_by_name["dashboard"]),
    ]
    cases.extend(("native", item) for item in examples_by_name["native"])

    try:
        import jsonschema  # type: ignore[import-not-found]

        backend = f"jsonschema {getattr(jsonschema, '__version__', 'available')}"
        validators = {}
        for key, schema in schemas.items():
            jsonschema.Draft202012Validator.check_schema(schema)
            validators[key] = jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker())
        validate = lambda key, instance: validators[key].validate(instance)
        rejects = lambda key, instance: not validators[key].is_valid(instance)
    except ImportError:
        backend = "stdlib subset validator (jsonschema unavailable)"
        validate = lambda key, instance: check_subset(schemas[key], instance, schemas[key])
        def rejects(key: str, instance: dict[str, Any]) -> bool:
            try:
                validate(key, instance)
            except ContractError:
                return True
            return False

    for key, instance in cases:
        try:
            validate(key, instance)
        except Exception as exc:
            print(f"FAIL valid {key} example: {exc}", file=sys.stderr)
            return 1

    invalid_count = 0
    for key, instance in unknown_mutations(examples_by_name):
        if not rejects(key, instance):
            print(f"FAIL unknown field accepted by {key} schema", file=sys.stderr)
            return 1
        invalid_count += 1

    print(f"Validated {len(cases)} synthetic examples with {backend}.")
    print(f"Rejected {invalid_count} synthetic unknown-field mutations, including nested structs.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
