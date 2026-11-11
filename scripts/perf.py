#!/usr/bin/env python3
"""Discovery performance on the first-party applications: median of N runs
(wall), peak RSS the maximum, plus worlds built, scenarios model-checked,
states and the report's own timings (`discover --model-check`).

Usage: scripts/perf.py [OUT_DIR] [--old BIN] [--new BIN] [--runs N] [--worlds]

OUT_DIR defaults to reports/perf. Default comparison: this build (--new,
default MIDWRITE_BIN or target/release/midwrite) cold (no extraction cache)
versus warm (`--cache` populated by one earlier run), and, with --old, an
earlier binary on the same applications (an application whose contracts the
earlier binary rejects is reported as not run).

--worlds instead compares world search off (`--max-worlds 1`) with the
default (8) on this build: worlds generated, invalid, duplicate and skipped,
time per candidate, and counterexamples found only in a non-base world.

Writes OUT_DIR/results.json and OUT_DIR/SUMMARY.md.
"""
import argparse, json, os, re, statistics, subprocess, sys, tempfile, time
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))
APPS = ["fixtures/eval/p13/eval-app", "fixtures/eval/p14/eval-app", "fixtures/eval/p15/eval-app",
        "fixtures/apps/worlds-app", "fixtures/apps/commerce-app", "fixtures/eval/p16/eval-app",
        "fixtures/apps/retry", "fixtures/apps/locks", "fixtures/apps/nulls",
        "fixtures/apps/genid", "fixtures/apps/zod/app"]

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
        # e.g. an earlier binary on a contracts file that uses newer formulas
        raise RuntimeError(next((l for l in p.stderr.splitlines() if "error" in l), f"exit {p.returncode}"))
    return secs, rss, json.loads(p.stdout)

def measure(binary, app, runs, cache=None, extra=()):
    args = ["discover", os.path.join(ROOT, app), "--model-check", "--format", "json", *extra]
    if cache:
        args += ["--cache", cache]
        timed(binary, args)  # populate
    walls, rsss, rep = [], [], None
    for _ in range(runs):
        w, r, rep = timed(binary, args)
        walls.append(w); rsss.append(r)
    b = rep["budgets"]; t = rep.get("timings", {})
    cands = rep["candidates"]
    first = [next((w for w in (c.get("coverage") or {}).get("worlds", []) if w.get("verdict") == "COUNTEREXAMPLE_FOUND"), None)
             for c in cands]
    return {
        "wallSecondsMedian": round(statistics.median(walls), 3), "peakRssMiB": round(max(rsss) / 2**20, 1),
        "candidates": len(cands), "counterexamples": rep["summary"]["modelCounterexamples"],
        "modelCheckable": rep["summary"]["modelCheckable"],
        "worldsBuilt": b.get("worldsBuilt"), "worldsInvalid": b.get("worldsInvalid"),
        "worldsDuplicate": b.get("worldsDuplicate"), "worldsSkippedForBudget": b.get("worldsSkippedForBudget"),
        "scenariosModelChecked": b["scenariosModelChecked"],
        "statesExplored": b["totalStatesExplored"], "synthesisMs": t.get("synthesisMs"),
        "modelCheckingMs": t.get("modelCheckingMs"), "totalMs": t.get("totalMs"),
        "msPerCandidate": round((t.get("totalMs") or 0) / max(1, len(cands)), 1),
        "counterexamplesInNonBaseWorlds": sum(1 for f in first if f and f["id"] != "base"),
    }

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out", nargs="?", default=os.path.join(ROOT, "reports", "perf"))
    ap.add_argument("--old", help="an earlier midwrite binary to compare with")
    ap.add_argument("--new", default=NEW); ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--worlds", action="store_true", help="world search off (--max-worlds 1) vs the default (8)")
    a = ap.parse_args(); os.makedirs(a.out, exist_ok=True)
    if a.worlds:
        configs = [("max-worlds 1", a.new, False, ["--max-worlds", "1"]), ("max-worlds 8", a.new, False, ["--max-worlds", "8"])]
        keys = ["app", "binary", "candidates", "counterexamples", "counterexamplesInNonBaseWorlds", "worldsBuilt",
                "worldsInvalid", "worldsDuplicate", "worldsSkippedForBudget", "scenariosModelChecked", "statesExplored",
                "synthesisMs", "modelCheckingMs", "msPerCandidate", "wallSecondsMedian", "peakRssMiB"]
        title = "# Discovery performance: world search off (1) vs default (8)"
        note = "timings from the report."
    else:
        configs = ([("old", a.old, False, [])] if a.old else []) + [("new cold", a.new, False, []), ("new warm", a.new, True, [])]
        keys = ["app", "binary", "candidates", "modelCheckable", "counterexamples", "worldsBuilt",
                "scenariosModelChecked", "statesExplored", "synthesisMs", "modelCheckingMs", "totalMs",
                "wallSecondsMedian", "peakRssMiB"]
        title = "# Discovery performance: " + ("old binary vs new binary (cold / warm cache)" if a.old else "cold / warm cache")
        note = "timings from the report. `warm`: `--cache` populated by one earlier run."
    rows = []
    for app in APPS:
        for label, binary, warm, extra in configs:
            with tempfile.TemporaryDirectory() as d:
                try:
                    row = measure(binary, app, a.runs, os.path.join(d, "cache.json") if warm else None, extra)
                except RuntimeError as e:
                    row = {"error": str(e)[:160]}
            row.update(app=app, binary=label)
            rows.append(row)
    with open(os.path.join(a.out, "results.json"), "w") as f:
        json.dump(rows, f, indent=1); f.write("\n")
    lines = [title, "",
             f"Median of {a.runs} runs (wall), peak RSS the maximum, `discover --model-check`; {note}", "",
             "| " + " | ".join(keys) + " |", "|" + " --- |" * len(keys)]
    for r in rows:
        lines.append("| " + " | ".join(str(r.get(k, "n/a")) for k in keys) + " |")
    for r in rows:
        if "error" in r:
            lines.append(f"\n* {r['app']} with {r['binary']}: not run ({r['error']})")
    open(os.path.join(a.out, "SUMMARY.md"), "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))

if __name__ == "__main__":
    main()
