#!/usr/bin/env python3
"""Phase 15 performance: discovery with world search off (`--max-worlds 1`,
the Phase 14 behavior) versus the default (8), median of N runs, peak RSS the
maximum; plus worlds generated, explored, states and time to the first
counterexample per candidate (from the report's own timings and coverage).

Usage: scripts/phase15-perf.py OUT_DIR [--runs N]
"""
import argparse, json, os, re, statistics, subprocess, sys, time
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))
APPS = ["fixtures/phase13/eval-app", "fixtures/phase14/eval-app", "fixtures/phase15/eval-app",
        "fixtures/phase15/worlds-app", "fixtures/phase11/commerce-app"]

def timed(args):
    t = ["/usr/bin/time", "-l"] if sys.platform == "darwin" else ["/usr/bin/time", "-v"]
    s = time.monotonic()
    p = subprocess.run(t + [BIN] + args, capture_output=True, text=True)
    secs = time.monotonic() - s
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
    rss = int(m.group(1)) if m else 0
    if sys.platform != "darwin":
        rss *= 1024
    return secs, rss, json.loads(p.stdout)

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("out"); ap.add_argument("--runs", type=int, default=3)
    a = ap.parse_args(); os.makedirs(a.out, exist_ok=True)
    rows = []
    for app in APPS:
        for worlds in (1, 8):
            walls, rsss, rep = [], [], None
            for _ in range(a.runs):
                w, r, rep = timed(["discover", os.path.join(ROOT, app), "--model-check", "--format", "json",
                                   "--max-worlds", str(worlds)])
                walls.append(w); rsss.append(r)
            b = rep["budgets"]; t = rep.get("timings", {})
            cands = rep["candidates"]
            first = [next((w for w in c["coverage"]["worlds"] if w.get("verdict") == "COUNTEREXAMPLE_FOUND"), None)
                     for c in cands if c.get("coverage")]
            rows.append({
                "app": app, "maxWorlds": worlds, "wallSecondsMedian": round(statistics.median(walls), 3),
                "peakRssMiB": round(max(rsss) / 2**20, 1), "candidates": len(cands),
                "counterexamples": rep["summary"]["modelCounterexamples"],
                "worldsBuilt": b.get("worldsBuilt"), "worldsInvalid": b.get("worldsInvalid"),
                "worldsDuplicate": b.get("worldsDuplicate"), "worldsSkippedForBudget": b.get("worldsSkippedForBudget"),
                "scenariosModelChecked": b["scenariosModelChecked"], "statesExplored": b["totalStatesExplored"],
                "synthesisMs": t.get("synthesisMs"), "modelCheckingMs": t.get("modelCheckingMs"), "totalMs": t.get("totalMs"),
                "msPerCandidate": round(t.get("totalMs", 0) / max(1, len(cands)), 1),
                "counterexamplesInNonBaseWorlds": sum(1 for f in first if f and f["id"] != "base"),
            })
    with open(os.path.join(a.out, "results.json"), "w") as f:
        json.dump(rows, f, indent=1); f.write("\n")
    keys = ["app", "maxWorlds", "candidates", "counterexamples", "counterexamplesInNonBaseWorlds", "worldsBuilt",
            "scenariosModelChecked", "statesExplored", "synthesisMs", "modelCheckingMs", "msPerCandidate",
            "wallSecondsMedian", "peakRssMiB"]
    lines = ["# Phase 15 performance: world search off (1) vs default (8)", "",
             f"Median of {a.runs} runs (wall), peak RSS the maximum, `discover --model-check`; timings from the report.", "",
             "| " + " | ".join(keys) + " |", "|" + " --- |" * len(keys)]
    for r in rows:
        lines.append("| " + " | ".join(str(r[k]) for k in keys) + " |")
    open(os.path.join(a.out, "SUMMARY.md"), "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))

main()
