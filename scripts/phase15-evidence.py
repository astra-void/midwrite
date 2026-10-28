#!/usr/bin/env python3
"""Phase 15 evidence: generated worlds under every search algorithm, on
PostgreSQL, and against the original code.

Usage: scripts/phase15-evidence.py OUT_DIR [--repeat N]

1. `commitrift discover --model-check --emit-scenarios` on four first-party
   applications (fixtures/phase13/eval-app, fixtures/phase14/eval-app,
   fixtures/phase15/eval-app, fixtures/phase15/worlds-app). Every emitted
   scenario (one per explored world and variant) is model-checked with the
   unreduced search, sleep sets (default), classic DPOR and source-set DPOR;
   verdict and completeness must agree.
2. Every scenario with a counterexample is replayed on PostgreSQL in
   lockstep (`--validate-postgres`): the database outcomes must match.
3. The deciding worlds of the worlds application (and the safe label
   counterpart) run against the original TypeScript (`--verify-source
   --allow-execution --source-repeat N`, first-party fixture only, disposable
   clusters).

Writes OUT_DIR/results.json and OUT_DIR/SUMMARY.md. COMMITRIFT_BIN selects
the binary.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))
APPS = [
    "fixtures/phase13/eval-app",
    "fixtures/phase14/eval-app",
    "fixtures/phase15/eval-app",
    "fixtures/phase15/worlds-app",
]
ALGORITHMS = [
    ("unreduced", ["--no-por"]),
    ("sleep-sets", []),
    ("classic-dpor", ["--dpor"]),
    ("source-dpor", ["--source-dpor"]),
]


def run(args):
    p = subprocess.run([BIN] + args, capture_output=True, text=True)
    try:
        return json.loads(p.stdout)
    except json.JSONDecodeError:
        return {"error": p.stderr.strip()[-400:]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--repeat", type=int, default=3)
    a = ap.parse_args()
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)
    results = {"binary": os.path.relpath(BIN, ROOT), "apps": {}, "por": [], "postgres": [], "source": []}
    for app in APPS:
        scen = os.path.join(out, "scenarios", os.path.basename(os.path.dirname(app)) + "-" + os.path.basename(app))
        shutil.rmtree(scen, ignore_errors=True)
        os.makedirs(scen)
        rep = run(["discover", os.path.join(ROOT, app), "--model-check", "--no-timing", "--format", "json",
                   "--emit-scenarios", scen])
        results["apps"][app] = {
            "candidates": len(rep.get("candidates", [])),
            "byStatus": rep.get("summary", {}).get("byStatus", {}),
            "budgets": rep.get("budgets", {}),
            "scenarios": scen,
        }
        if app.endswith("worlds-app"):
            results["worldsReport"] = rep
        for name in sorted(os.listdir(scen)):
            if not name.endswith(".json") or name.endswith(".counterexample.json"):
                continue
            path = os.path.join(scen, name)
            row = {"app": app, "scenario": name, "algorithms": {}}
            for label, extra in ALGORITHMS:
                mc = run(["model-check", path, "--format", "json", "--no-timing"] + extra)
                ex = mc.get("exploration", {})
                row["algorithms"][label] = {
                    "verdict": ex.get("verdict", mc.get("error")),
                    "complete": ex.get("complete"),
                    "states": ex.get("metrics", {}).get("states"),
                }
            base = row["algorithms"]["unreduced"]
            row["agree"] = all(
                (v["verdict"], v["complete"]) == (base["verdict"], base["complete"])
                for v in row["algorithms"].values()
            )
            results["por"].append(row)
            if base["verdict"] == "COUNTEREXAMPLE_FOUND":
                mc = run(["model-check", path, "--validate-postgres", "--format", "json", "--no-timing"])
                rt = mc.get("runtime", {})
                results["postgres"].append({
                    "app": app, "scenario": name, "status": rt.get("status"),
                    "observations": len(rt.get("observations", [])),
                    "disagreements": sum(1 for o in rt.get("observations", []) if not o.get("agree", True)),
                    "evidence": mc.get("evidence", {}).get("level"),
                })
    # Original code: the worlds application's deciding worlds and the label
    # counterpart.
    worlds = results.pop("worldsReport", {})
    scen = results["apps"]["fixtures/phase15/worlds-app"]["scenarios"]
    for c in worlds.get("candidates", []):
        entry = c["entries"][0]["selector"].split("#")[1]
        s = next((s for s in c["scenarios"] if s.get("model", {}).get("verdict") == "COUNTEREXAMPLE_FOUND"), None)
        if not s or not s.get("file"):
            continue
        mc = run(["model-check", os.path.join(scen, s["file"]), "--verify-source", "--allow-execution",
                  "--source-repeat", str(a.repeat), "--format", "json", "--no-timing"])
        sv = mc.get("sourceVerification", {})
        results["source"].append({
            "entry": entry, "candidateStatus": c["status"], "world": s.get("world", {}).get("id"),
            "scenario": s["file"], "status": sv.get("status"), "evidence": mc.get("evidence", {}).get("level"),
            "runs": [{k: r.get(k) for k in ("statementsAgree", "tablesAgree", "outcomesAgree", "invariant", "reproduced")}
                     for r in sv.get("runs", [])],
        })
    for r in results["apps"].values():
        r["scenarios"] = os.path.relpath(r["scenarios"], ROOT)
    with open(os.path.join(out, "results.json"), "w") as f:
        json.dump(results, f, indent=1, sort_keys=True)
        f.write("\n")
    agree = sum(1 for r in results["por"] if r["agree"])
    lines = [
        "# Phase 15 evidence: generated worlds", "",
        f"Binary `{results['binary']}`. Generated by `scripts/phase15-evidence.py`.", "",
        "## Applications", "",
        "| application | candidates | statuses | worlds built | worlds invalid | worlds duplicate | worlds skipped (budget) |",
        "| --- | ---: | --- | ---: | ---: | ---: | ---: |",
    ]
    for app, r in results["apps"].items():
        b = r["budgets"]
        lines.append(f"| `{app}` | {r['candidates']} | {r['byStatus']} | {b.get('worldsBuilt')} | {b.get('worldsInvalid')} | {b.get('worldsDuplicate')} | {b.get('worldsSkippedForBudget')} |")
    lines += ["", "## Search algorithms (every emitted scenario)", "",
              f"{agree} of {len(results['por'])} scenarios: unreduced, sleep sets, classic DPOR and source-set DPOR agree on verdict and completeness.", ""]
    for r in results["por"]:
        if not r["agree"]:
            lines.append(f"* DISAGREEMENT `{r['scenario']}`: {r['algorithms']}")
    pg_ok = sum(1 for r in results["postgres"] if r["status"] == "confirmed")
    lines += ["## PostgreSQL lockstep (scenarios with a counterexample)", "",
              f"{pg_ok} of {len(results['postgres'])} confirmed; database outcomes compared: {sum(r['observations'] for r in results['postgres'])}, disagreements: {sum(r['disagreements'] for r in results['postgres'])}.", ""]
    for r in results["postgres"]:
        if r["status"] != "confirmed":
            lines.append(f"* `{r['scenario']}`: {r['status']} ({r['evidence']})")
    lines += ["", "## Original code (fixtures/phase15/worlds-app)", "",
              "| entry | candidate status | world | original code | evidence | runs reproduced |", "| --- | --- | --- | --- | --- | --- |"]
    for r in results["source"]:
        rep = sum(1 for x in r["runs"] if x.get("reproduced"))
        lines.append(f"| `{r['entry']}` | {r['candidateStatus']} | `{r['world']}` | {r['status']} | {r['evidence']} | {rep}/{len(r['runs'])} |")
    with open(os.path.join(out, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    bad = agree != len(results["por"]) or any(
        r["status"] != "reproduced" for r in results["source"] if r["candidateStatus"] == "MODEL_COUNTEREXAMPLE")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
