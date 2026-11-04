#!/usr/bin/env python3
"""Derive the model-check report schema v5 (Phase 10) from v4 (Phase 9).

v5 only *adds*: source-set DPOR (`bounds.reduction: "source-dpor"`,
`bounds.sourceDporMode`, `exploration.reductionMetrics`) and connection losses
under a live process (`sourceVerification.recovery.losses`, the evidence
`ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED` / `original-code-connection-loss-
reproduced`). A report uses schema 5 only when one of these is present; every
other report keeps schema 1-4. Writes docs/phase10/schemas/model-check-report.v5.schema.json.
"""
import copy
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
v4 = json.loads((ROOT / "docs/phase9/schemas/model-check-report.v4.schema.json").read_text())
s = copy.deepcopy(v4)
s["$id"] = "commitrift.model-check.v5"
s["title"] = "Midwrite model-check report, schema 5"
s["description"] = (
    "Phase 10: adds source-set dynamic partial-order reduction and connection losses under a live "
    "process. Everything of schema 4 is unchanged."
)
P = s["properties"]
P["schemaVersion"] = {"const": 5}
nonneg = {"type": "integer", "minimum": 0}
string_array = {"type": "array", "items": {"type": "string"}}
fates = {"enum": ["not-reached", "answered-and-withheld", "held-for-later-delivery"]}

P["bounds"]["properties"]["reduction"] = {"enum": ["dpor", "source-dpor"]}
P["bounds"]["properties"]["sourceDporMode"] = {"enum": ["stateful-sleep", "stateful", "stateless-sleep"]}
P["exploration"]["properties"]["reductionMetrics"] = {
    "type": "object",
    "additionalProperties": False,
    "required": ["races", "backtrackPoints", "summaryChecks", "summaries", "descriptors",
                 "summaryBytes", "sleepPrunedTransitions", "maxFrames"],
    "properties": {k: nonneg for k in ["races", "backtrackPoints", "summaryChecks", "summaries", "descriptors",
                                        "summaryBytes", "sleepPrunedTransitions", "maxFrames"]},
}
P["evidence"]["properties"]["level"]["enum"].append("original-code-connection-loss-reproduced")
sv = P["sourceVerification"]["properties"]
sv["evidence"]["enum"].insert(1, "ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED")
rows = {"type": "object", "additionalProperties": {"type": "array", "items": {"$ref": "#/definitions/row"}}}
sv["recovery"]["properties"]["losses"] = {
    "type": "array",
    "items": {
        "type": "object",
        "additionalProperties": False,
        "required": [
            "transition", "actor", "attempt", "operation", "modelFate", "observedFate", "mechanism",
            "forwardedToServer", "withheldFromServer", "clientSevered", "workerPid", "workerAliveAfter",
            "workerConnectedAfter", "sessionEnded", "sessionOpen", "applicationKnowledge", "physicalOutcome",
            "status",
        ],
        "properties": {
            "transition": nonneg,
            "actor": {"type": "string"},
            "attempt": {"type": "integer", "minimum": 1},
            "operation": {"type": "string"},
            "modelFate": fates,
            "observedFate": {"enum": fates["enum"] + [""]},
            "mechanism": {"type": "string"},
            "forwardedToServer": {"type": "boolean"},
            "withheldFromServer": {"type": "boolean"},
            "serverAnswer": {"type": "string"},
            "clientSevered": {"type": "boolean"},
            "workerPid": nonneg,
            "workerAliveAfter": {"type": "boolean"},
            "workerConnectedAfter": {"type": "boolean"},
            "driverError": {"type": "string"},
            "driverSqlstate": {"type": "string"},
            "backendStateBefore": {"type": "string"},
            "backendPid": {"type": "integer"},
            "sessionEnded": {"type": "boolean"},
            "sessionOpen": {"type": "boolean"},
            "deadConnectionStatements": string_array,
            "laterBackendPids": {"type": "array", "items": {"type": "integer"}},
            "wireEvents": string_array,
            "applicationKnowledge": {"type": "string"},
            "physicalOutcome": {"type": "string"},
            "modelTables": rows,
            "runtimeTables": rows,
            "status": {"enum": ["agree", "differ", "not-observed"]},
            "differences": string_array,
        },
    },
}

out = ROOT / "docs/phase10/schemas/model-check-report.v5.schema.json"
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(s, indent=2) + "\n")
print(f"wrote {out}")
