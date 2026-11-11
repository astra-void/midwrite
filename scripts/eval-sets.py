#!/usr/bin/env python3
"""Score `midwrite discover` on the frozen first-party evaluation sets
(fixtures/eval/pNN), stage by stage.

Usage:
  scripts/eval-sets.py SET [OUT.json] [--first]
  scripts/eval-sets.py SET --verify-only

SET is one of p11 p12 p13 p14 p15 p16, or `all` (OUT is then a directory
that receives <set>.json). OUT defaults to reports/eval/sets/<set>.json.
MIDWRITE_BIN selects the binary (an earlier binary scores the same set for
comparison).

--first       refuse to run when OUT already exists: the first scoring of a
              set is recorded once, before any change prompted by it, and
              never overwritten.
--verify-only check the set's FROZEN.json checksums and exit; does not run
              midwrite.

Freeze check (before any run). FROZEN.json `files` are paths relative to the
set's directory:
  p13            every listed file must still have its recorded sha256;
  p14, p15, p16  in addition no listed file may be missing, no file may be
                 added under eval-app/, and the manifest digest (sha256 of
                 the lines '<sha256>  <path>\\n' sorted by path) must match
                 `manifestSha256`;
  p11, p12       have no FROZEN.json; the label file's sha256 is recorded
                 in the output instead.

Stages (each measured on its own; precision/recall only where labels say
positive/negative):
  schema      tables modeled vs expected (p12: also generated-key columns)
  entries     labeled entries extracted
  effects     labeled entries with a bound database write (`effectful`)
  candidates  entries with a candidate of the labeled class
  model       the tool's own evaluation block (report["evaluation"])
  validity    counterexamples that replay and violate their invariant
  original    original-code reproduction: NOT measured (the evaluation
              applications have no authorized harness)

Scoring (`scoring`), model prediction:
  * an unpaired case is predicted positive when the entry has a candidate of
    its own (every actor is the entry) with status MODEL_COUNTEREXAMPLE and
    every such counterexample is validated (replays and violates);
  * a paired case (`pairedWith`) is predicted positive when a candidate whose
    actors are exactly the two entries has a MODEL_COUNTEREXAMPLE (p14 and
    later: a validated one).
  p12, p13: TP/FP/FN/TN, precision and recall over the decided cases (`all`).
  p14, p15, p16: also without the cases whose property must be declared and
  the application does not declare (`requiresDeclaration` and not
  `declaredInContracts`: `excludingUndeclared`), and per family (`byFamily`).
  p15, p16: each positive with its safe counterpart (`counterpart`, reporting
  only): whether both were classified correctly (`counterparts`).
  Unsupported entries are reported separately (`unsupported`), never counted
  as TP/FP/FN/TN.

p14 and later: if the binary rejects the label file (no JSON report on
stdout), the run is repeated once with a copy reduced to the P13 label
fields, and the output says so (`labelsPassedToTool`); the full labels are
still used here for scoring.

p11 (the discovery benchmark): the commerce application against
fixtures/eval/p11/labels.json and the held-out labels of the team and shop
applications (fixtures/eval/p11/heldout), scored by the tool itself
(`--labels`, report["evaluation"]), plus the commerce application without
its contracts file.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))
EVAL = os.path.join(ROOT, "fixtures", "eval")
SETS = ["p11", "p12", "p13", "p14", "p15", "p16"]
# How each set is checked and scored (see the module docstring).
SPEC = {
    "p11": {"freeze": None, "scorer": "tool"},
    "p12": {"freeze": None, "scorer": "stages", "generated": True},
    "p13": {"freeze": "files", "scorer": "stages", "pairs": True},
    "p14": {"freeze": "strict", "scorer": "families"},
    "p15": {"freeze": "strict", "scorer": "families", "counterparts": True},
    "p16": {"freeze": "strict", "scorer": "families", "counterparts": True},
}

# The P13 label format (fixtures/eval/p13/eval-labels.json); used only for
# the fallback copy of p14 and later.
P13_TOP_KEYS = {"$comment", "schemaVersion", "project", "expectedSchema", "cases", "corrections"}
P13_SCHEMA_KEYS = {"tables", "generators", "foreignKeys", "uniqueKeys", "replicaHandles"}
P13_CASE_KEYS = {"id", "entry", "class", "label", "effectful", "tables", "property", "environment", "why",
                 "pairedWith", "observable", "replicas"}

SAFE_STATUSES = {"NO_VIOLATION_WITHIN_BOUNDS"}
IGNORED_FILES = {".DS_Store"}


def rel(path):
    return os.path.relpath(path, ROOT)


def sha256_file(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


# --- freeze -----------------------------------------------------------------

def verify_frozen(name):
    """Check the set's FROZEN.json; exit on a violation. Returns a summary."""
    base = os.path.join(EVAL, name)
    mode = SPEC[name]["freeze"]
    if mode is None:
        return {"frozen": None, "note": "this set has no FROZEN.json"}
    path = os.path.join(base, "FROZEN.json")
    frozen = json.load(open(path))
    files = frozen["files"]
    changed, missing = [], []
    for rel_path, digest in files.items():
        p = os.path.join(base, rel_path)
        if not os.path.exists(p):
            missing.append(rel_path)
        elif sha256_file(p) != digest:
            changed.append(rel_path)
    if mode == "files":
        if changed or missing:
            sys.exit(f"{name}: frozen files changed: {changed + missing}")
        return {"frozen": rel(path), "frozenFilesVerified": len(files)}
    present = set()
    for dirpath, _dirs, names in os.walk(os.path.join(base, "eval-app")):
        for n in names:
            if n in IGNORED_FILES:
                continue
            present.add(os.path.relpath(os.path.join(dirpath, n), base).replace(os.sep, "/"))
    added = sorted(present - set(files))
    manifest = "".join(f"{files[r]}  {r}\n" for r in sorted(files))
    manifest_ok = hashlib.sha256(manifest.encode()).hexdigest() == frozen["manifestSha256"]
    if changed or missing or added or not manifest_ok:
        sys.exit(f"{name}: frozen set violated: changed={changed} missing={missing} added={added} "
                 f"manifestOk={manifest_ok}")
    return {"frozen": rel(path), "frozenFilesVerified": len(files),
            "frozenManifestSha256": frozen["manifestSha256"]}


# --- running the tool -------------------------------------------------------

def run_json(args):
    p = subprocess.run([BIN, *args], capture_output=True, text=True)
    try:
        return json.loads(p.stdout), p
    except json.JSONDecodeError:
        return None, p


def discover_labeled(app, labels_path):
    report, p = run_json(["discover", app, "--model-check", "--labels", labels_path,
                          "--format", "json", "--no-timing"])
    if report is None:
        sys.exit(f"midwrite produced no JSON report: exit {p.returncode}: {p.stderr[-4000:]}")
    return report


def sanitized_labels(labels):
    out = {k: v for k, v in labels.items() if k in P13_TOP_KEYS}
    out["expectedSchema"] = {k: v for k, v in labels["expectedSchema"].items() if k in P13_SCHEMA_KEYS}
    out["cases"] = [{k: v for k, v in c.items() if k in P13_CASE_KEYS} for c in labels["cases"]]
    return out


def discover_with_fallback(app, labels_path, labels):
    report, p = run_json(["discover", app, "--model-check", "--labels", labels_path,
                          "--format", "json", "--no-timing"])
    if report is not None:
        return report, "original", None
    first_error = (p.returncode, p.stderr[-4000:])
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "eval-labels.p13-fields.json")
        with open(path, "w") as f:
            json.dump(sanitized_labels(labels), f)
        report, p2 = run_json(["discover", app, "--model-check", "--labels", path,
                               "--format", "json", "--no-timing"])
    if report is None:
        sys.exit(f"midwrite produced no JSON report.\n"
                 f"with the frozen labels: exit {first_error[0]}: {first_error[1]}\n"
                 f"with the P13 fields only: exit {p2.returncode}: {p2.stderr[-4000:]}")
    return report, "sanitized-p13-fields", {"exit": first_error[0], "stderr": first_error[1]}


# --- scoring ----------------------------------------------------------------

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


def family_scoring(rows, counterparts):
    """Overall, excluding undeclared properties, per family (p14 and later)."""
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
    out = {
        "definition": "model prediction: an unpaired case is positive when the entry's own candidate has a validated MODEL_COUNTEREXAMPLE; a paired case when the pair's candidate has one",
        "all": confusion(decided),
        "excludingUndeclared": dict(confusion(declared_or_inferable), excluded=undeclared),
        "byFamily": by_family,
    }
    if counterparts:
        by_id = {r["id"]: r for r in rows}
        pairs = []
        for r in rows:
            twin = by_id.get(r.get("counterpart"))
            if r["label"] == "positive" and twin is not None:
                pairs.append({"positive": r["id"], "negative": twin["id"], "family": r["family"],
                              "outcomes": [r["outcome"], twin["outcome"]],
                              "bothRight": r["outcome"] == "TP" and twin["outcome"] == "TN"})
        out["counterparts"] = {"pairs": pairs, "bothRight": sum(p["bothRight"] for p in pairs)}
    return out


def staged_app(app, tmp):
    """The frozen applications predate the Midwrite name: their contracts file
    is `commitrift-contracts.json` with kind `commitrift.contracts`, which
    the tool no longer reads. The frozen bytes are never edited; discovery
    runs on a copy whose contracts file carries the current name and kind."""
    legacy = os.path.join(app, "commitrift-contracts.json")
    if not os.path.exists(legacy) or os.path.exists(os.path.join(app, "midwrite-contracts.json")):
        return app
    copy = os.path.join(tmp, "eval-app")
    shutil.copytree(app, copy)
    os.remove(os.path.join(copy, "commitrift-contracts.json"))
    doc = json.load(open(legacy))
    if doc.get("kind") == "commitrift.contracts":
        doc["kind"] = "midwrite.contracts"
    with open(os.path.join(copy, "midwrite-contracts.json"), "w") as f:
        json.dump(doc, f, indent=2)
    return copy


def score_set(name):
    """p12 to p16: run discovery on the set's application and score it."""
    spec = SPEC[name]
    base = os.path.join(EVAL, name)
    labels_path = os.path.join(base, "eval-labels.json")
    labels = json.load(open(labels_path))
    families = spec["scorer"] == "families"
    labels_mode, first_error = "original", None
    with tempfile.TemporaryDirectory() as staging:
        app = staged_app(os.path.join(base, "eval-app"), staging)
        if families:
            report, labels_mode, first_error = discover_with_fallback(app, labels_path, labels)
        else:
            report = discover_labeled(app, labels_path)
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
    if spec.get("generated"):
        gen = {g[0]: g[1] for g in index.get("generatedColumns", [])}
        want = {k: v for k, v in exp["generators"].items() if v in ("database-generated", "application-generated")}
        stages["schema"]["generatedExpected"] = len(want)
        stages["schema"]["generatedRecognized"] = sum(1 for k, v in want.items() if gen.get(k) == v.split("-")[0])
    rows = []
    for case in labels["cases"]:
        sel = case["entry"]
        e = entries.get(sel)
        mine = own(sel)
        of_class = [c for c in mine if c["class"] == case["class"]]
        cex = [c for c in mine if c["status"] == "MODEL_COUNTEREXAMPLE"]
        row = {
            "id": case.get("id"), "pairedWith": case.get("pairedWith"),
            "entry": sel, "label": case["label"], "class": case["class"],
            "extracted": e is not None and e["status"] not in ("extraction-failed", "not-extracted"),
            "status": e["status"] if e else "absent",
            "effectful": bool(e and e.get("writes")),
            "unknownOps": e.get("unknown", None) if e else None,
            "candidateOfClass": bool(of_class),
            "candidates": [(c["class"], c["status"]) for c in mine],
            "counterexample": bool(cex),
            "validated": all_validated(cex),
        }
        if families:
            row.update({
                "family": case["family"],
                "requiresDeclaration": bool(case.get("requiresDeclaration")),
                "declaredInContracts": bool(case.get("declaredInContracts")),
                "classAgrees": any(c["status"] == "MODEL_COUNTEREXAMPLE" for c in of_class),
            })
        if spec.get("counterparts"):
            row.update({"counterpart": case.get("counterpart"), "inputSensitivity": case.get("inputSensitivity")})
        rows.append(row)
    # Paired cases (two different entries): a candidate whose actors are
    # exactly the pair, and its status.
    for r in rows:
        if r["pairedWith"] and (families or spec.get("pairs")):
            pair = {r["entry"], r["pairedWith"]}
            mine = [c for c in cands if {x["selector"] for x in c["entries"]} == pair]
            pcex = [c for c in mine if c["status"] == "MODEL_COUNTEREXAMPLE"]
            r["pairCandidates"] = [(c["class"], c["status"]) for c in mine]
            r["pairCounterexample"] = bool(pcex)
            if families:
                r["pairValidated"] = all_validated(pcex)
                r["predicted"] = r["pairCounterexample"] and r["pairValidated"]
            else:
                r["predicted"] = r["pairCounterexample"]
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
    if families:
        scoring = family_scoring(rows, spec.get("counterparts"))
    else:
        scoring = {"definition": "model prediction: an unpaired case is positive when the entry's own candidate has a validated MODEL_COUNTEREXAMPLE; a paired case when the pair's candidate has a MODEL_COUNTEREXAMPLE",
                   "all": confusion(decided)}
    result = {"set": name, "labelsSha256": sha256_file(labels_path),
              "labelsPassedToTool": labels_mode, "labelsRejectedWith": first_error,
              "binary": BIN, "stages": stages, "scoring": scoring, "cases": rows,
              "summary": report.get("summary"),
              "index": {k: v for k, v in index.items() if k != "transactionHelpers"}}
    brief = {"stages": stages, "scoring": {k: v for k, v in scoring.items() if k != "byFamily"}}
    if families:
        brief["byFamily"] = {k: {m: v[m] for m in ("TP", "FP", "FN", "TN")} for k, v in scoring["byFamily"].items()}
    return result, brief


def score_p11():
    """The discovery benchmark: the tool's own evaluation of p11."""
    base = os.path.join(EVAL, "p11")
    app = os.path.join(ROOT, "fixtures", "apps", "commerce-app")
    labels_path = os.path.join(base, "labels.json")
    r = discover_labeled(app, labels_path)
    e = r["evaluation"]
    result = {"set": "p11", "binary": BIN, "labelsSha256": sha256_file(labels_path),
              "firstParty": {"application": rel(app), "evaluation": e, "summary": r["summary"]},
              "heldout": {}}
    held_path = os.path.join(base, "heldout", "labels.json")
    held = json.load(open(held_path))
    result["heldoutLabelsSha256"] = sha256_file(held_path)
    with tempfile.TemporaryDirectory() as tmp:
        for name, p in held["projects"].items():
            lab = os.path.join(tmp, f"{name}-labels.json")
            with open(lab, "w") as f:
                json.dump({"cases": p["cases"]}, f)
            hr, proc = run_json(["discover", os.path.join(ROOT, p["root"]), "--contracts",
                                 os.path.join(ROOT, p["contracts"]), "--model-check", "--no-timing",
                                 "--labels", lab, "--format", "json"])
            if hr is None:
                sys.exit(f"midwrite produced no JSON report for {name}: {proc.stderr[-4000:]}")
            result["heldout"][name] = {"application": p["root"], "evaluation": hr["evaluation"]}
        # The same application without its contracts file.
        copy = os.path.join(tmp, "no-contracts")
        shutil.copytree(app, copy, ignore=shutil.ignore_patterns("node_modules", "dist"))
        os.remove(os.path.join(copy, "midwrite-contracts.json"))
        nr, proc = run_json(["discover", copy, "--model-check", "--no-timing", "--format", "json"])
        result["withoutContracts"] = nr["summary"] if nr else {"error": proc.stderr[-400:]}
    brief = {"firstParty": {k: e.get(k) for k in ("staticHazardPrediction", "modelPrediction", "classAgreement",
                                                  "unsupportedReported", "unsupportedMissed", "candidateToModel")},
             "heldout": {n: h["evaluation"].get("modelPrediction") for n, h in result["heldout"].items()},
             "withoutContracts": result["withoutContracts"]}
    return result, brief


def run_set(name, out, first):
    if first and os.path.exists(out):
        sys.exit(f"{out} exists: the first scoring is recorded once")
    freeze = verify_frozen(name)
    if SPEC[name]["scorer"] == "tool":
        result, brief = score_p11()
    else:
        result, brief = score_set(name)
    result.update(freeze)
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w") as f:
        json.dump(result, f, indent=1, sort_keys=True)
        f.write("\n")
    print(json.dumps({"set": name, **brief}, indent=1, sort_keys=True))


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    unknown = flags - {"--first", "--verify-only", "--help", "-h"}
    if "--help" in flags or "-h" in flags:
        print(__doc__)
        return
    if not args or unknown or args[0] not in SETS + ["all"] or len(args) > 2:
        sys.exit(__doc__ if not unknown else f"unknown option {sorted(unknown)}\n{__doc__}")
    sets = SETS if args[0] == "all" else [args[0]]
    if "--verify-only" in flags:
        for name in sets:
            print(name, json.dumps(verify_frozen(name), sort_keys=True))
        return
    default_dir = os.path.join(ROOT, "reports", "eval", "sets")
    for name in sets:
        if args[0] == "all":
            out = os.path.join(args[1] if len(args) > 1 else default_dir, f"{name}.json")
        else:
            out = args[1] if len(args) > 1 else os.path.join(default_dir, f"{name}.json")
        run_set(name, out, "--first" in flags)


if __name__ == "__main__":
    main()
