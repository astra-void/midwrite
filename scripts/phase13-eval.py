#!/usr/bin/env python3
"""Phase 13 evaluation: score discovery on fixtures/phase13/eval-app against
the frozen labels (fixtures/phase13/eval-labels.json), stage by stage.

Usage: scripts/phase13-eval.py OUT.json [--first]

The checksums in fixtures/phase13/FROZEN.json are verified first: a changed
application or label file refuses the run. COMMITRIFT_BIN selects the
binary (the Phase 12 baseline binary scores the same set for comparison).

With --first, refuses to run when the first scoring already exists (it is
recorded once, before any change prompted by it, and never overwritten).

Stages (each measured on its own; precision/recall only where labels say
positive/negative):
  schema      tables modeled vs expected; generated-key columns vs expected
  entries     labeled entries extracted
  effects     labeled entries with a bound database write (`effectful`)
  candidates  entries with a candidate of the labeled class
  model       the scorer's model prediction (validated counterexample)
  validity    counterexamples that replay and violate their invariant
  original    original-code reproduction: NOT measured (the evaluation
              application has no authorized harness)
"""
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))
APP = os.path.join(ROOT, "fixtures", "phase13", "eval-app")
LABELS = os.path.join(ROOT, "fixtures", "phase13", "eval-labels.json")
FROZEN = os.path.join(ROOT, "fixtures", "phase13", "FROZEN.json")


def verify_frozen():
    import hashlib
    frozen = json.load(open(FROZEN))
    base = os.path.join(ROOT, "fixtures", "phase13")
    bad = []
    for rel, digest in frozen["files"].items():
        with open(os.path.join(base, rel), "rb") as f:
            if hashlib.sha256(f.read()).hexdigest() != digest:
                bad.append(rel)
    if bad:
        sys.exit(f"frozen files changed: {bad}")
    return len(frozen["files"])


def main():
    out = sys.argv[1]
    if "--first" in sys.argv and os.path.exists(out):
        sys.exit(f"{out} exists: the first scoring is recorded once")
    frozen_files = verify_frozen()
    labels = json.load(open(LABELS))
    p = subprocess.run([BIN, "discover", APP, "--model-check", "--labels", LABELS,
                        "--format", "json", "--no-timing"], capture_output=True, text=True)
    report = json.loads(p.stdout)
    entries = {e["selector"]: e for e in report["entries"]}
    cands = report["candidates"]

    def own(sel):
        return [c for c in cands if all(x["selector"] == sel for x in c["entries"])]

    stages = {}
    exp = labels["expectedSchema"]
    modeled = report["index"]["tablesModeled"]
    stages["schema"] = {
        "tablesExpected": len(exp["tables"]), "tablesModeled": modeled,
        "tablesRefused": [t[0] for t in report["index"]["tablesRefused"]],
    }
    rows = []
    for case in labels["cases"]:
        sel = case["entry"]
        e = entries.get(sel)
        mine = own(sel)
        of_class = [c for c in mine if c["class"] == case["class"]]
        cex = [c for c in mine if c["status"] == "MODEL_COUNTEREXAMPLE"]
        rows.append({
            "id": case.get("id"), "pairedWith": case.get("pairedWith"),
            "entry": sel, "label": case["label"], "class": case["class"],
            "extracted": e is not None and e["status"] not in ("extraction-failed", "not-extracted"),
            "status": e["status"] if e else "absent",
            "effectful": bool(e and e["writes"]),
            "unknownOps": e.get("unknown", None) if e else None,
            "candidateOfClass": bool(of_class),
            "candidates": [(c["class"], c["status"]) for c in mine],
            "counterexample": bool(cex),
            "validated": all((sc.get("model") or {}).get("validation") is None
                             or (sc["model"]["validation"]["replayed"] and sc["model"]["validation"]["violates"])
                             for c in cex for sc in c.get("scenarios", [])
                             if (sc.get("model") or {}).get("verdict") == "COUNTEREXAMPLE_FOUND"),
        })
    stages["entries"] = {"labeled": len(rows), "extracted": sum(r["extracted"] for r in rows)}
    stages["effects"] = {"expectedEffectful": sum(1 for c in labels["cases"] if c["effectful"]),
                         "effectful": sum(r["effectful"] for r in rows)}
    decided = [r for r in rows if r["label"] in ("positive", "negative")]
    stages["candidates"] = {"positives": sum(r["label"] == "positive" for r in decided),
                            "positivesWithCandidateOfClass": sum(r["candidateOfClass"] for r in decided if r["label"] == "positive"),
                            "negativesWithCandidateOfClass": sum(r["candidateOfClass"] for r in decided if r["label"] == "negative")}
    stages["model"] = report.get("evaluation")
    stages["validity"] = {"entriesWithCounterexample": sum(r["counterexample"] for r in rows),
                          "allValidated": all(r["validated"] for r in rows)}
    stages["original"] = "not measured: the evaluation application has no authorized harness"
    # Paired cases (two different entries): a candidate whose actors are
    # exactly the pair, and its status.
    for r in rows:
        if r["pairedWith"]:
            pair = {r["entry"], r["pairedWith"]}
            mine = [c for c in cands if {x["selector"] for x in c["entries"]} == pair]
            r["pairCandidates"] = [(c["class"], c["status"]) for c in mine]
            r["pairCounterexample"] = any(c["status"] == "MODEL_COUNTEREXAMPLE" for c in mine)
    unsupported = [r for r in rows if r["label"] == "unsupported"]
    stages["unsupported"] = [{"entry": r["entry"], "status": r["status"], "candidates": r["candidates"]} for r in unsupported]
    result = {"frozen": "fixtures/phase13/FROZEN.json", "frozenFilesVerified": frozen_files,
              "binary": BIN, "stages": stages, "cases": rows,
              "summary": report["summary"], "index": {k: v for k, v in report["index"].items() if k != "transactionHelpers"}}
    with open(out, "w") as f:
        json.dump(result, f, indent=1, sort_keys=True)
        f.write("\n")
    print(json.dumps(stages, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
