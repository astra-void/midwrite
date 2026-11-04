#!/usr/bin/env python3
"""Derive the model-check report schema v4 (Phase 9) from v3 (Phase 8).

v4 only *adds*: every v3 report with the same content is valid v4 except for
the `schemaVersion` constant. Run from anywhere; writes
docs/phase9/schemas/model-check-report.v4.schema.json.
"""
import copy
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
v3 = json.loads((ROOT / "docs/phase8/schemas/model-check-report.v3.schema.json").read_text())
s = copy.deepcopy(v3)
s["$id"] = "commitrift.model-check.v4"
s["title"] = "Midwrite model-check report, schema 4"
s["description"] = (
    "Phase 9: adds the in-flight operation evidence, the application-knowledge (twin world) analysis, "
    "stuck-state observations, the redacted protocol log, shared-process and pool facts. Everything "
    "of schema 3 is unchanged."
)
P = s["properties"]
P["schemaVersion"] = {"const": 4}

string_array = {"type": "array", "items": {"type": "string"}}
nonneg = {"type": "integer", "minimum": 0}
worlds = {"enum": ["committed", "notCommitted"]}
verdicts = {
    "type": "array",
    "items": {
        "type": "object",
        "additionalProperties": False,
        "required": ["invariant", "holds"],
        "properties": {"invariant": {"type": "string"}, "holds": {"type": "boolean"}},
    },
}

P["epistemic"] = {
    "type": "object",
    "additionalProperties": False,
    "required": ["twins"],
    "properties": {
        "twins": {
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": False,
                "required": [
                    "transition", "actor", "operation", "actual", "other", "knowledge", "twin",
                    "observations", "identicalObservations", "reading",
                ],
                "properties": {
                    "transition": {"type": "integer", "minimum": 1},
                    "actor": {"type": "string"},
                    "operation": {"type": "string"},
                    "actual": worlds,
                    "other": worlds,
                    "knowledge": {"type": "string"},
                    "twin": {"enum": ["replayed", "diverges"]},
                    "detail": {"type": "string"},
                    "actualVerdicts": verdicts,
                    "twinVerdicts": verdicts,
                    "observations": nonneg,
                    "identicalObservations": nonneg,
                    "firstDifference": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 2},
                    "reading": {"type": "string"},
                },
            },
        }
    },
}

declared = P["bounds"]["properties"]["declared"]["properties"]
declared["maxInFlight"] = nonneg
declared["crashPolicy"] = {"enum": ["any", "in-flight-only"]}
P["bounds"]["properties"]["reduction"] = {"enum": ["dpor"]}
P["exploration"]["properties"]["stuck"] = string_array
P["exploration"]["properties"]["storage"] = {
    "type": "object",
    "additionalProperties": False,
    "required": ["storedStates", "fragments", "fragmentReferences", "keyArenaBytes", "tableBytes", "internerBytes", "edgeBytes", "nodeBytes", "bytesPerState"],
    "properties": {
        "storedStates": nonneg, "fragments": nonneg, "fragmentReferences": nonneg, "keyArenaBytes": nonneg,
        "tableBytes": nonneg, "internerBytes": nonneg, "edgeBytes": nonneg, "nodeBytes": nonneg,
        "bytesPerState": {"type": "number", "minimum": 0},
    },
}
P["evidence"]["properties"]["level"]["enum"].append("original-code-in-flight-failure-reproduced")
step_kind = P["counterexample"]["properties"]["steps"]["items"]["properties"]["kind"]["enum"]
step_kind += ["ack-lost-committed", "ack-lost-not-committed", "crash-in-flight", "server-complete"]
s["definitions"]["event"]["properties"]["kind"]["enum"] += ["ackLost", "serverCompletion"]

sv = P["sourceVerification"]["properties"]
sv["evidence"]["enum"].insert(1, "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED")
sv["wireLog"] = string_array
sv["sharedProcesses"] = string_array
sv["stuck"] = {
    "type": "object",
    "additionalProperties": False,
    "required": ["modelKind", "observed", "status"],
    "properties": {
        "modelKind": {"enum": ["pool-starvation", "application-wait-cycle"]},
        "status": {"enum": ["agree", "differ"]},
        "observed": {
            "type": "object",
            "additionalProperties": False,
            "required": ["kind", "quietMs", "unfinished", "confirmed", "detail"],
            "properties": {
                "kind": {"type": "string"},
                "quietMs": nonneg,
                "unfinished": string_array,
                "pool": {"type": "array", "items": {"type": "integer"}, "minItems": 3, "maxItems": 3},
                "lockWaits": nonneg,
                "confirmed": {"type": "boolean"},
                "detail": {"type": "string"},
            },
        },
    },
}
rec = sv["recovery"]["properties"]
rec["completions"] = {
    "type": "array",
    "items": {
        "type": "object",
        "additionalProperties": False,
        "required": ["transition", "actor", "attempt", "serverAnswer", "sessionEnded", "status"],
        "properties": {
            "transition": nonneg,
            "actor": {"type": "string"},
            "attempt": {"type": "integer", "minimum": 1},
            "serverAnswer": {"type": "string"},
            "sessionEnded": {"type": "boolean"},
            "modelTables": {"type": "object", "additionalProperties": {"type": "array", "items": {"$ref": "#/definitions/row"}}},
            "runtimeTables": {"type": "object", "additionalProperties": {"type": "array", "items": {"$ref": "#/definitions/row"}}},
            "status": {"enum": ["agree", "differ", "not-observed"]},
            "differences": string_array,
        },
    },
}
crash = rec["crashes"]["items"]["properties"]
crash["inFlight"] = {
    "type": "object",
    "additionalProperties": False,
    "required": [
        "operation", "modelFate", "observedFate", "mechanism", "forwardedToServer", "withheldFromServer",
        "clientAcknowledged", "sessionOpenAfterKill", "applicationKnowledge", "physicalOutcome",
    ],
    "properties": {
        "operation": {"type": "string"},
        "modelFate": {"enum": ["not-reached", "answered-and-withheld", "held-for-later-delivery"]},
        "observedFate": {"enum": ["not-reached", "answered-and-withheld", "held-for-later-delivery"]},
        "mechanism": {"type": "string"},
        "forwardedToServer": {"type": "boolean"},
        "withheldFromServer": {"type": "boolean"},
        "serverAnswer": {"type": "string"},
        "clientAcknowledged": {"type": "boolean"},
        "backendStateBeforeKill": {"type": "string"},
        "sessionOpenAfterKill": {"type": "boolean"},
        "wireEvents": string_array,
        "applicationKnowledge": {"type": "string"},
        "physicalOutcome": {"type": "string"},
    },
}

out = ROOT / "docs/phase9/schemas/model-check-report.v4.schema.json"
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(s, indent=2) + "\n")
print(f"wrote {out}")
