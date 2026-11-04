#!/usr/bin/env python3
"""Phase 16 evidence (derived from scripts/phase15-evidence.py): generated
worlds under every search algorithm, on PostgreSQL, and against the original
code, now including the Phase 16 applications (zod, loops/retry, multi-row
locks, NULLS NOT DISTINCT, generated identities, the frozen Phase 16 set).

Usage: scripts/phase16-evidence.py OUT_DIR [--repeat N]

1. `midwrite discover --model-check --emit-scenarios` on the first-party
   applications listed in APPS. Every emitted
   scenario (one per explored world and variant) is model-checked with the
   unreduced search, sleep sets (default), classic DPOR and source-set DPOR;
   verdict and completeness must agree.
2. Every scenario with a counterexample is replayed on PostgreSQL in
   lockstep (`--validate-postgres`): the database outcomes must match.
3. The deciding worlds of every candidate with a model counterexample in
   the applications that ship an authorized harness (SOURCE_APPS) run
   against the original TypeScript (`--verify-source --allow-execution
   --source-repeat N`, first-party fixtures only, disposable clusters). Safe
   counterparts are covered by recovery-probe tests
   (crates/midwrite-cli/tests/phase15.rs, phase16_loops.rs).

Writes OUT_DIR/results.json and OUT_DIR/SUMMARY.md. MIDWRITE_BIN selects
the binary.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))
APPS = [
    "fixtures/phase13/eval-app",
    "fixtures/phase14/eval-app",
    "fixtures/phase15/eval-app",
    "fixtures/phase15/worlds-app",
    "fixtures/phase16/eval-app",
    "fixtures/phase16/zod/app",
    "fixtures/phase16/retry",
    "fixtures/phase16/locks",
    "fixtures/phase16/nulls",
    "fixtures/phase16/genid",
]
SOURCE_APPS = ["fixtures/phase15/worlds-app", "fixtures/phase16/retry"]
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
        scen = os.path.join(out, "scenarios", app.replace("fixtures/", "").replace("/", "-"))
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
        if app in SOURCE_APPS:
            results.setdefault("sourceReports", {})[app] = rep
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
    # Original code: deciding worlds of the harnessed applications.
    for app, rep in results.pop("sourceReports", {}).items():
        scen = results["apps"][app]["scenarios"]
        for c in rep.get("candidates", []):
            entry = c["entries"][0]["selector"].split("#")[1]
            s = next((s for s in c["scenarios"] if s.get("model", {}).get("verdict") == "COUNTEREXAMPLE_FOUND"), None)
            if not s or not s.get("file"):
                continue
            mc = run(["model-check", os.path.join(scen, s["file"]), "--verify-source", "--allow-execution",
                      "--source-repeat", str(a.repeat), "--format", "json", "--no-timing"])
            sv = mc.get("sourceVerification", {})
            results["source"].append({
                "app": app, "entry": entry, "candidateStatus": c["status"], "class": c.get("class"),
                "world": (s.get("world") or {}).get("id"), "scenario": s["file"], "status": sv.get("status"),
                "evidence": mc.get("evidence", {}).get("level"),
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
        "# Phase 16 evidence: generated worlds, new semantics", "",
        f"Binary `{results['binary']}`. Generated by `scripts/phase16-evidence.py`.", "",
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
    lines += ["", "## Original code (" + ", ".join(f"`{x}`" for x in SOURCE_APPS) + ")", "",
              "| application | entry | class | candidate status | world | original code | evidence | runs reproduced |", "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    for r in results["source"]:
        rep = sum(1 for x in r["runs"] if x.get("reproduced"))
        lines.append(f"| `{r['app']}` | `{r['entry']}` | {r['class']} | {r['candidateStatus']} | `{r['world']}` | {r['status']} | {r['evidence']} | {rep}/{len(r['runs'])} |")
    with open(os.path.join(out, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    # Non-reproduction is reported, not a failure of this script: a
    # counterexample against a heuristic hypothesis, or one the harness
    # cannot drive, is recorded with its status. Disagreements fail it.
    bad = agree != len(results["por"]) or sum(r["disagreements"] for r in results["postgres"]) > 0
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
