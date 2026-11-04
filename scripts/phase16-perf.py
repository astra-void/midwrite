#!/usr/bin/env python3
"""Phase 16 performance (derived from scripts/phase15-perf.py): the Phase 15
binary versus the Phase 16 binary on the same applications, and the Phase 16
binary cold (no extraction cache) versus warm (`--cache` already populated).
Median of N runs (wall), peak RSS the maximum, plus worlds built, scenarios
model-checked, states and the report's own timings.

Usage: scripts/phase16-perf.py OUT_DIR --old BIN [--new BIN] [--runs N]
"""
import argparse, json, os, re, statistics, subprocess, sys, tempfile, time
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))
APPS = ["fixtures/phase13/eval-app", "fixtures/phase14/eval-app", "fixtures/phase15/eval-app",
        "fixtures/phase15/worlds-app", "fixtures/phase11/commerce-app", "fixtures/phase16/eval-app",
        "fixtures/phase16/retry", "fixtures/phase16/locks", "fixtures/phase16/nulls",
        "fixtures/phase16/genid", "fixtures/phase16/zod/app"]

def timed(binary, args):
    t = ["/usr/bin/time", "-l"] if sys.platform == "darwin" else ["/usr/bin/time", "-v"]
    s = time.monotonic()
    p = subprocess.run(t + [binary] + args, capture_output=True, text=True)
    secs = time.monotonic() - s
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
    rss = int(m.group(1)) if m else 0
    if sys.platform != "darwin":
        rss *= 1024
    if p.returncode != 0 and not p.stdout.strip():
        # e.g. the Phase 15 binary on a contracts file that uses Phase 16 formulas
        raise RuntimeError(next((l for l in p.stderr.splitlines() if "error" in l), f"exit {p.returncode}"))
    return secs, rss, json.loads(p.stdout)

def measure(binary, app, runs, cache=None):
    args = ["discover", os.path.join(ROOT, app), "--model-check", "--format", "json"]
    if cache:
        args += ["--cache", cache]
        timed(binary, args)  # populate
    walls, rsss, rep = [], [], None
    for _ in range(runs):
        w, r, rep = timed(binary, args)
        walls.append(w); rsss.append(r)
    b = rep["budgets"]; t = rep.get("timings", {})
    return {
        "wallSecondsMedian": round(statistics.median(walls), 3), "peakRssMiB": round(max(rsss) / 2**20, 1),
        "candidates": len(rep["candidates"]), "counterexamples": rep["summary"]["modelCounterexamples"],
        "modelCheckable": rep["summary"]["modelCheckable"],
        "worldsBuilt": b.get("worldsBuilt"), "scenariosModelChecked": b["scenariosModelChecked"],
        "statesExplored": b["totalStatesExplored"], "synthesisMs": t.get("synthesisMs"),
        "modelCheckingMs": t.get("modelCheckingMs"), "totalMs": t.get("totalMs"),
    }

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("out"); ap.add_argument("--old", required=True)
    ap.add_argument("--new", default=NEW); ap.add_argument("--runs", type=int, default=3)
    a = ap.parse_args(); os.makedirs(a.out, exist_ok=True)
    rows = []
    for app in APPS:
        for label, binary, warm in (("phase15", a.old, False), ("phase16 cold", a.new, False), ("phase16 warm", a.new, True)):
            with tempfile.TemporaryDirectory() as d:
                try:
                    row = measure(binary, app, a.runs, os.path.join(d, "cache.json") if warm else None)
                except RuntimeError as e:
                    row = {"error": str(e)[:160]}
            row.update(app=app, binary=label)
            rows.append(row)
    with open(os.path.join(a.out, "results.json"), "w") as f:
        json.dump(rows, f, indent=1); f.write("\n")
    keys = ["app", "binary", "candidates", "modelCheckable", "counterexamples", "worldsBuilt",
            "scenariosModelChecked", "statesExplored", "synthesisMs", "modelCheckingMs", "totalMs",
            "wallSecondsMedian", "peakRssMiB"]
    lines = ["# Phase 16 performance: Phase 15 binary vs Phase 16 binary (cold / warm cache)", "",
             f"Median of {a.runs} runs (wall), peak RSS the maximum, `discover --model-check`; timings from the report. "
             "`warm`: `--cache` populated by one earlier run.", "",
             "| " + " | ".join(keys) + " |", "|" + " --- |" * len(keys)]
    for r in rows:
        lines.append("| " + " | ".join(str(r.get(k, "n/a")) for k in keys) + " |")
    for r in rows:
        if "error" in r:
            lines.append(f"\n* {r['app']} with {r['binary']}: not run ({r['error']})")
    open(os.path.join(a.out, "SUMMARY.md"), "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))

main()
