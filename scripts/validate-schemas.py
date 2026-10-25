#!/usr/bin/env python3
"""Validate CommitRift JSON reports against their published JSON Schemas.

    scripts/validate-schemas.py [--sarif-schema FILE] REPORT...

The schema is chosen by the report's `kind` and `schemaVersion` (SARIF files
by their `version`). Every report is also checked to be *rejected* after a
deliberate corruption, so that a schema that accepts anything cannot pass.
Uses the `jsonschema` package. Without it, `--builtin` selects a small
built-in validator for the draft-07 subset the CommitRift schemas use; it
refuses (exit 2) any schema keyword it does not implement, so it can never
accept a report by ignoring a rule. Exit status 1 on any error.
"""

import argparse
import copy
import json
import pathlib
import sys

try:
    import jsonschema
except ImportError:  # pragma: no cover
    jsonschema = None

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCHEMAS = {
    ("commitrift.traces", 1): ROOT / "docs/phase3/schemas/trace-report.v1.schema.json",
    ("commitrift.traces", 2): ROOT / "docs/phase5/schemas/trace-report.v2.schema.json",
    ("commitrift.verification", 1): ROOT / "docs/phase3/schemas/verification-report.v1.schema.json",
    ("commitrift.verification", 2): ROOT / "docs/phase4/schemas/verification-report.v2.schema.json",
    ("commitrift.regression", 1): ROOT / "docs/phase4/schemas/regression-report.v1.schema.json",
    ("commitrift.model-check", 1): ROOT / "docs/phase6/schemas/model-check-report.v1.schema.json",
    ("commitrift.model-check", 2): ROOT / "docs/phase7/schemas/model-check-report.v2.schema.json",
}


def registry():
    """Every CommitRift schema by its `$id` (they reference each other)."""
    from referencing import Registry, Resource
    from referencing.jsonschema import DRAFT7

    resources = []
    for path in SCHEMAS.values():
        schema = json.loads(path.read_text())
        resources.append((schema["$id"], Resource.from_contents(schema, default_specification=DRAFT7)))
    return Registry().with_resources(resources)


def corrupt(report):
    """A copy with one structural error."""
    bad = copy.deepcopy(report)
    if "runs" in bad and isinstance(bad["runs"], list):  # SARIF
        bad["runs"][0]["results"] = "not-an-array"
    else:
        bad["schemaVersion"] = "one"
    return bad


SUPPORTED = {"$schema", "$id", "title", "description", "definitions", "$ref", "type", "properties",
             "required", "additionalProperties", "items", "minItems", "maxItems", "enum", "const",
             "minimum", "pattern"}


def builtin_errors(schema, instance, root, path=()):
    """Errors of `instance` against a draft-07 subset; raises on unknown keywords."""
    import re
    unknown = set(schema) - SUPPORTED
    if unknown:
        raise SystemExit(f"builtin validator: unsupported schema keywords {sorted(unknown)}; install jsonschema")
    if "$ref" in schema:
        name = schema["$ref"].removeprefix("#/definitions/")
        yield from builtin_errors(root["definitions"][name], instance, root, path)
        return
    types = schema.get("type")
    if types is not None:
        types = [types] if isinstance(types, str) else types
        def is_type(t):
            return {
                "object": isinstance(instance, dict), "array": isinstance(instance, list),
                "string": isinstance(instance, str), "boolean": isinstance(instance, bool),
                "integer": isinstance(instance, int) and not isinstance(instance, bool),
                "null": instance is None,
                "number": isinstance(instance, (int, float)) and not isinstance(instance, bool),
            }[t]
        if not any(is_type(t) for t in types):
            yield (path, f"{instance!r} is not of type {types}")
            return
    if "const" in schema and instance != schema["const"]:
        yield (path, f"{instance!r} is not {schema['const']!r}")
    if "enum" in schema and instance not in schema["enum"]:
        yield (path, f"{instance!r} is not one of {schema['enum']}")
    if "minimum" in schema and isinstance(instance, int) and instance < schema["minimum"]:
        yield (path, f"{instance} < {schema['minimum']}")
    if "pattern" in schema and isinstance(instance, str) and not re.search(schema["pattern"], instance):
        yield (path, f"{instance!r} does not match {schema['pattern']}")
    if isinstance(instance, dict):
        for key in schema.get("required", []):
            if key not in instance:
                yield (path, f"{key!r} is a required property")
        props = schema.get("properties", {})
        for key, value in instance.items():
            if key in props:
                yield from builtin_errors(props[key], value, root, path + (key,))
            else:
                extra = schema.get("additionalProperties", True)
                if extra is False:
                    yield (path, f"additional property {key!r}")
                elif isinstance(extra, dict):
                    yield from builtin_errors(extra, value, root, path + (key,))
    if isinstance(instance, list):
        if "minItems" in schema and len(instance) < schema["minItems"]:
            yield (path, f"fewer than {schema['minItems']} items")
        if "maxItems" in schema and len(instance) > schema["maxItems"]:
            yield (path, f"more than {schema['maxItems']} items")
        items = schema.get("items")
        if isinstance(items, list):
            for i, (sub, value) in enumerate(zip(items, instance)):
                yield from builtin_errors(sub, value, root, path + (i,))
        elif isinstance(items, dict):
            for i, value in enumerate(instance):
                yield from builtin_errors(items, value, root, path + (i,))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sarif-schema", type=pathlib.Path)
    parser.add_argument("--builtin", action="store_true", help="use the built-in subset validator")
    parser.add_argument("reports", nargs="+", type=pathlib.Path)
    args = parser.parse_args()
    failed = False
    for path in args.reports:
        report = json.loads(path.read_text())
        if report.get("$schema", "").find("sarif") >= 0 or report.get("version") == "2.1.0":
            if not args.sarif_schema:
                print(f"{path}: SARIF, but no --sarif-schema given", file=sys.stderr)
                failed = True
                continue
            schema_path = args.sarif_schema
            label = "SARIF 2.1.0"
            count = sum(len(run.get("results", [])) for run in report.get("runs", []))
            unit = "results"
        else:
            key = (report.get("kind"), report.get("schemaVersion"))
            schema_path = SCHEMAS.get(key)
            if schema_path is None:
                print(f"{path}: no schema for kind/version {key}", file=sys.stderr)
                failed = True
                continue
            label = schema_path.name.replace(".schema.json", "")
            for unit in ("traces", "cases", "entries", "actors"):
                if unit in report:
                    count = len(report[unit])
                    break
            else:
                count, unit = 0, "items"
        schema = json.loads(schema_path.read_text())
        if args.builtin or jsonschema is None:
            if not args.builtin:
                sys.exit("validate-schemas.py needs the `jsonschema` package (pip install jsonschema), or --builtin")
            found = list(builtin_errors(schema, report, schema))
            errors = [type("E", (), {"path": list(p), "message": m}) for p, m in found]
            rejected = any(True for _ in builtin_errors(schema, corrupt(report), schema))
            label += " (builtin validator)"
        else:
            validator_class = jsonschema.validators.validator_for(schema)
            validator = validator_class(schema, registry=registry())
            errors = sorted(validator.iter_errors(report), key=lambda e: list(e.path))
            rejected = any(True for _ in validator.iter_errors(corrupt(report)))
        print(f"{label}: {path.name}: {count} {unit}, {len(errors)} schema errors; corrupted copy rejected: {rejected}")
        for error in errors[:5]:
            print(f"  at {'/'.join(map(str, error.path))}: {error.message[:200]}")
        if errors or not rejected:
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
