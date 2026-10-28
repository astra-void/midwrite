#!/usr/bin/env python3
"""Phase 14 evaluation: score discovery on fixtures/phase14/eval-app against
the frozen labels (fixtures/phase14/eval-labels.json), stage by stage, overall
and per source-construct family.

Usage: scripts/phase14-eval.py OUT.json [--first]

The checksums in fixtures/phase14/FROZEN.json are verified first: a changed,
missing or added application file, a changed label file, or a manifest digest
that does not match refuses the run. COMMITRIFT_BIN selects the binary (an
earlier binary scores the same set for comparison).

With --first, refuses to run when OUT.json already exists: the first scoring
is recorded once, before any change prompted by it, and never overwritten.

Stages (each measured on its own; precision/recall only where labels say
positive/negative):
  schema      tables modeled vs expected
  entries     labeled entries extracted
  effects     labeled entries with a bound database write (`effectful`)
  candidates  entries with a candidate of the labeled class
  model       the tool's own evaluation block (report["evaluation"])
  validity    counterexamples that replay and violate their invariant
  original    original-code reproduction: NOT measured (the evaluation
              application has no authorized harness)

Scoring computed here (`scoring`), model prediction:
  * an unpaired case is predicted positive when the entry has a candidate of
    its own (every actor is the entry) with status MODEL_COUNTEREXAMPLE and
    every such counterexample is validated (replays and violates);
  * a paired case (`pairedWith`) is predicted positive when a candidate whose
    actors are exactly the two entries has a validated MODEL_COUNTEREXAMPLE.
  TP/FP/FN/TN, precision and recall: overall (`all`), without the
  return-value cases whose property this application does not declare
  (`requiresDeclaration` and not `declaredInContracts`:
  `excludingUndeclared`), and per family (`byFamily`). Unsupported entries are
  reported separately (`unsupported`), never counted as TP/FP/FN/TN.

If the binary rejects the label file (no JSON report on stdout), the run is
repeated once with a copy reduced to the Phase 13 label fields, and the
output says so (`labelsPassedToTool`); the Phase 14 fields are still used
here for scoring.
"""
import hashlib
import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))
BASE = os.path.join(ROOT, "fixtures", "phase14")
APP = os.path.join(BASE, "eval-app")
LABELS = os.path.join(BASE, "eval-labels.json")
FROZEN = os.path.join(BASE, "FROZEN.json")

# The Phase 13 label format (fixtures/phase13/eval-labels.json); used only for
# the fallback copy.
PHASE13_TOP_KEYS = {"$comment", "schemaVersion", "project", "expectedSchema", "cases", "corrections"}
PHASE13_SCHEMA_KEYS = {"tables", "generators", "foreignKeys", "uniqueKeys", "replicaHandles"}
PHASE13_CASE_KEYS = {"id", "entry", "class", "label", "effectful", "tables", "property", "environment", "why",
                     "pairedWith", "observable", "replicas"}

SAFE_STATUSES = {"NO_VIOLATION_WITHIN_BOUNDS"}
IGNORED_FILES = {".DS_Store"}


def sha256_file(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def verify_frozen():
    frozen = json.load(open(FROZEN))
    files = frozen["files"]
    changed, missing = [], []
    for rel, digest in files.items():
        path = os.path.join(BASE, rel)
        if not os.path.exists(path):
            missing.append(rel)
        elif sha256_file(path) != digest:
            changed.append(rel)
    present = set()
    for dirpath, _dirs, names in os.walk(APP):
        for name in names:
            if name in IGNORED_FILES:
                continue
            present.add(os.path.relpath(os.path.join(dirpath, name), BASE).replace(os.sep, "/"))
    added = sorted(present - set(files))
    manifest = "".join(f"{files[rel]}  {rel}\n" for rel in sorted(files))
    manifest_ok = hashlib.sha256(manifest.encode()).hexdigest() == frozen["manifestSha256"]
    if changed or missing or added or not manifest_ok:
        sys.exit(f"frozen set violated: changed={changed} missing={missing} added={added} manifestOk={manifest_ok}")
    return frozen


def sanitized_labels(labels):
    out = {k: v for k, v in labels.items() if k in PHASE13_TOP_KEYS}
    out["expectedSchema"] = {k: v for k, v in labels["expectedSchema"].items() if k in PHASE13_SCHEMA_KEYS}
    out["cases"] = [{k: v for k, v in c.items() if k in PHASE13_CASE_KEYS} for c in labels["cases"]]
    return out


def run_discover(labels_path):
    p = subprocess.run([BIN, "discover", APP, "--model-check", "--labels", labels_path,
                        "--format", "json", "--no-timing"], capture_output=True, text=True)
    try:
        return json.loads(p.stdout), p
    except json.JSONDecodeError:
        return None, p


def discover(labels):
    report, p = run_discover(LABELS)
    if report is not None:
        return report, "original", None
    first_error = (p.returncode, p.stderr[-4000:])
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "eval-labels.phase13-fields.json")
        with open(path, "w") as f:
            json.dump(sanitized_labels(labels), f)
        report, p2 = run_discover(path)
    if report is None:
        sys.exit(f"commitrift produced no JSON report.\n"
                 f"with the frozen labels: exit {first_error[0]}: {first_error[1]}\n"
                 f"with the Phase 13 fields only: exit {p2.returncode}: {p2.stderr[-4000:]}")
    return report, "sanitized-phase13-fields", {"exit": first_error[0], "stderr": first_error[1]}


def all_validated(candidates):
    return all((sc.get("model") or {}).get("validation") is None
               or (sc["model"]["validation"]["replayed"] and sc["model"]["validation"]["violates"])
               for c in candidates for sc in c.get("scenarios", [])
               if (sc.get("model") or {}).get("verdict") == "COUNTEREXAMPLE_FOUND")


def confusion(rows):
    tp = sum(1 for r in rows if r["label"] == "positive" and r["predicted"])
    fp = sum(1 for r in rows if r["label"] == "negative" and r["predicted"])
    fn = sum(1 for r in rows if r["label"] == "positive" and not r["predicted"])
    tn = sum(1 for r in rows if r["label"] == "negative" and not r["predicted"])
    return {"TP": tp, "FP": fp, "FN": fn, "TN": tn,
            "precision": round(tp / (tp + fp), 3) if tp + fp else None,
            "recall": round(tp / (tp + fn), 3) if tp + fn else None}


def outcome(r):
    if r["label"] == "unsupported":
        return "unsupported"
    if r["label"] == "positive":
        return "TP" if r["predicted"] else "FN"
    return "FP" if r["predicted"] else "TN"


def score(rows):
    """Overall, excluding undeclared return-value properties, and per family."""
    decided = [r for r in rows if r["label"] in ("positive", "negative")]
    undeclared = [r["id"] for r in decided if r["requiresDeclaration"] and not r["declaredInContracts"]]
    declared_or_inferable = [r for r in decided if r["id"] not in undeclared]
    by_family = {}
    for fam in sorted({r["family"] for r in rows}):
        mine = [r for r in rows if r["family"] == fam]
        entry = {"cases": len(mine),
                 "positive": sum(r["label"] == "positive" for r in mine),
                 "negative": sum(r["label"] == "negative" for r in mine),
                 "unsupported": sum(r["label"] == "unsupported" for r in mine)}
        entry.update(confusion([r for r in mine if r["label"] in ("positive", "negative")]))
        entry["outcomes"] = {r["id"]: r["outcome"] for r in mine}
        by_family[fam] = entry
    return {
        "definition": "model prediction: an unpaired case is positive when the entry's own candidate has a validated MODEL_COUNTEREXAMPLE; a paired case when the pair's candidate has one",
        "all": confusion(decided),
        "excludingUndeclared": dict(confusion(declared_or_inferable), excluded=undeclared),
        "byFamily": by_family,
    }


def main():
    if len(sys.argv) < 2 or sys.argv[1].startswith("--"):
        sys.exit(__doc__)
    out = sys.argv[1]
    if "--first" in sys.argv and os.path.exists(out):
        sys.exit(f"{out} exists: the first scoring is recorded once")
    frozen = verify_frozen()
    labels = json.load(open(LABELS))
    report, labels_mode, first_error = discover(labels)
    entries = {e["selector"]: e for e in report["entries"]}
    cands = report["candidates"]

    def own(sel):
        return [c for c in cands if all(x["selector"] == sel for x in c["entries"])]

    stages = {}
    exp = labels["expectedSchema"]
    index = report.get("index", {})
    stages["schema"] = {
        "tablesExpected": len(exp["tables"]), "tablesModeled": index.get("tablesModeled"),
        "tablesRefused": [t[0] for t in index.get("tablesRefused", [])],
    }
    rows = []
    for case in labels["cases"]:
        sel = case["entry"]
        e = entries.get(sel)
        mine = own(sel)
        of_class = [c for c in mine if c["class"] == case["class"]]
        cex = [c for c in mine if c["status"] == "MODEL_COUNTEREXAMPLE"]
        rows.append({
            "id": case.get("id"), "family": case["family"], "pairedWith": case.get("pairedWith"),
            "entry": sel, "label": case["label"], "class": case["class"],
            "requiresDeclaration": bool(case.get("requiresDeclaration")),
            "declaredInContracts": bool(case.get("declaredInContracts")),
            "extracted": e is not None and e["status"] not in ("extraction-failed", "not-extracted"),
            "status": e["status"] if e else "absent",
            "effectful": bool(e and e.get("writes")),
            "unknownOps": e.get("unknown", None) if e else None,
            "candidateOfClass": bool(of_class),
            "candidates": [(c["class"], c["status"]) for c in mine],
            "counterexample": bool(cex),
            "validated": all_validated(cex),
            "classAgrees": any(c["status"] == "MODEL_COUNTEREXAMPLE" for c in of_class),
        })
    # Paired cases (two different entries): a candidate whose actors are
    # exactly the pair, and its status.
    for r in rows:
        if r["pairedWith"]:
            pair = {r["entry"], r["pairedWith"]}
            mine = [c for c in cands if {x["selector"] for x in c["entries"]} == pair]
            pcex = [c for c in mine if c["status"] == "MODEL_COUNTEREXAMPLE"]
            r["pairCandidates"] = [(c["class"], c["status"]) for c in mine]
            r["pairCounterexample"] = bool(pcex)
            r["pairValidated"] = all_validated(pcex)
            r["predicted"] = r["pairCounterexample"] and r["pairValidated"]
        else:
            r["predicted"] = r["counterexample"] and r["validated"]
        r["outcome"] = outcome(r)
    stages["entries"] = {"labeled": len(rows), "extracted": sum(r["extracted"] for r in rows)}
    stages["effects"] = {"expectedEffectful": sum(1 for c in labels["cases"] if c["effectful"]),
                         "effectful": sum(r["effectful"] for r in rows)}
    decided = [r for r in rows if r["label"] in ("positive", "negative")]
    stages["candidates"] = {"positives": sum(r["label"] == "positive" for r in decided),
                            "positivesWithCandidateOfClass": sum(r["candidateOfClass"] for r in decided if r["label"] == "positive"),
                            "negativesWithCandidateOfClass": sum(r["candidateOfClass"] for r in decided if r["label"] == "negative")}
    stages["model"] = report.get("evaluation")
    stages["validity"] = {"entriesWithCounterexample": sum(r["counterexample"] for r in rows),
                          "allValidated": all(r["validated"] for r in rows)
                          and all(r.get("pairValidated", True) for r in rows)}
    stages["original"] = "not measured: the evaluation application has no authorized harness"
    unsupported = [r for r in rows if r["label"] == "unsupported"]
    stages["unsupported"] = [{"id": r["id"], "entry": r["entry"], "status": r["status"],
                              "unknownOps": r["unknownOps"], "candidates": r["candidates"],
                              "claimedSafe": any(s in SAFE_STATUSES for _, s in r["candidates"])}
                             for r in unsupported]
    result = {"frozen": "fixtures/phase14/FROZEN.json", "frozenFilesVerified": len(frozen["files"]),
              "frozenManifestSha256": frozen["manifestSha256"],
              "labelsSha256": sha256_file(LABELS),
              "labelsPassedToTool": labels_mode, "labelsRejectedWith": first_error,
              "binary": BIN, "stages": stages, "scoring": score(rows), "cases": rows,
              "summary": report.get("summary"),
              "index": {k: v for k, v in index.items() if k != "transactionHelpers"}}
    with open(out, "w") as f:
        json.dump(result, f, indent=1, sort_keys=True)
        f.write("\n")
    print(json.dumps({"stages": stages, "scoring": {k: v for k, v in result["scoring"].items() if k != "byFamily"},
                      "byFamily": {k: {m: v[m] for m in ("TP", "FP", "FN", "TN")}
                                   for k, v in result["scoring"]["byFamily"].items()}},
                     indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
