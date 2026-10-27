#!/usr/bin/env python3
"""Phase 12 evidence: discovery on the first-party workspace application,
original-code runs of its generated counterexamples, and the model's verdicts
for their safe counterparts.

Usage: scripts/phase12-evidence.py [--repeat N]

Writes reports/phase12/evidence/. Application code runs ONLY for the
first-party workspace application (`--verify-source --allow-execution`).
Exits 1 unless every counterexample with a harness mapping reproduced in
every run.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))
OUT = os.path.join(ROOT, "reports", "phase12", "evidence")
APP = os.path.join(ROOT, "fixtures", "phase12", "workspace-app")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=3)
    args = ap.parse_args()
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    scen = os.path.join(OUT, "scenarios")
    os.makedirs(scen)
    p = subprocess.run([BIN, "discover", APP, "--model-check", "--emit-scenarios", scen,
                        "--format", "json", "--no-timing"], capture_output=True, text=True)
    report = json.loads(p.stdout)
    with open(os.path.join(OUT, "workspace-app.json"), "w") as f:
        json.dump(report, f, indent=1, sort_keys=True)
    for fmt in ("text", "html"):
        q = subprocess.run([BIN, "discover", APP, "--model-check", "--format", fmt, "--no-timing"],
                           capture_output=True, text=True)
        with open(os.path.join(OUT, f"workspace-app.{'txt' if fmt == 'text' else 'html'}"), "w") as f:
            f.write(q.stdout)
    lines = ["# Phase 12 evidence: workspace application", "",
             "| candidate | class | entries | status | variant | model | runtime evidence | reproduced |",
             "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    failures = 0
    for c in report["candidates"]:
        entries = ", ".join(e["selector"].split("#")[1] for e in c["entries"])
        for sc in c.get("scenarios", []):
            model = (sc.get("model") or {}).get("verdict", "-")
            level, ok = "-", "-"
            if model == "COUNTEREXAMPLE_FOUND" and sc["variant"]["name"] == "actual" and sc.get("file") \
                    and sc.get("sourceVerification"):
                path = os.path.join(scen, sc["file"])
                r = subprocess.run([BIN, "model-check", path, "--verify-source", "--allow-execution",
                                    "--source-repeat", str(args.repeat), "--format", "json", "--no-timing"],
                                   capture_output=True, text=True)
                mc = json.loads(r.stdout)
                with open(os.path.join(OUT, "runtime-" + sc["file"]), "w") as f:
                    json.dump(mc, f, indent=1, sort_keys=True)
                runs = (mc.get("sourceVerification") or {}).get("runs", [])
                good = sum(1 for x in runs if x.get("reproduced"))
                level = mc["evidence"]["level"]
                ok = f"{good}/{len(runs)}"
                if good != args.repeat or not level.startswith("original-code-"):
                    failures += 1
            lines.append(f"| {c['id']} | {c['class']} | {entries} | {c['status']} | {sc['variant']['name']} | {model} | {level} | {ok} |")
    with open(os.path.join(OUT, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
