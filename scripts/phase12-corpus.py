#!/usr/bin/env python3
"""Phase 12 real-world coverage and performance, Phase 11 binary vs Phase 12.

Usage:
  scripts/phase12-corpus.py OUT_DIR --old BIN [--runs N] [--private DIR ...]

Static discovery with bounded model checking on .corpora/saas-starter and
.corpora/midday (public, pinned) and every --private DIR (a `git archive` of
a pinned private revision; only aggregate counts are written). No project is
executed. Writes OUT_DIR/corpus-<name>-<phase>.json and OUT_DIR/SUMMARY.md.
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))


def timed(cmd):
    t = ["/usr/bin/time", "-l"] if sys.platform == "darwin" else ["/usr/bin/time", "-v"]
    start = time.monotonic()
    p = subprocess.run(t + cmd, capture_output=True, text=True)
    secs = time.monotonic() - start
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    rss = int(m.group(1)) if m else None
    if rss is None:
        m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
        rss = int(m.group(1)) * 1024 if m else None
    return p.returncode, secs, rss, p.stdout


def mib(n):
    return f"{n / 1048576:.0f} MiB" if n else "?"


def run(binary, path, *extra):
    code, secs, rss, out = timed([binary, "discover", path, "--format", "json", "--model-check", *extra])
    return json.loads(out), secs, rss


def aggregate(report, secs, rss, public):
    i, s = report["index"], report["summary"]
    agg = {
        "modules": report["project"]["modules"],
        "entryCandidates": i["entryCandidates"],
        "entriesExtracted": i["entriesExtracted"],
        "effectfulEntries": i["effectfulEntries"],
        "tablesModeled": i["tablesModeled"],
        "tablesRefused": len(i["tablesRefused"]),
        "generatedColumns": len(i.get("generatedColumns", [])),
        "unknownOperations": i["unknownOperations"],
        "unknownBreakdown": s.get("unknownOperationBreakdown", {}),
        "candidates": s["candidates"],
        "modelCheckable": s["modelCheckable"],
        "counterexamples": s["modelCounterexamples"],
        "statuses": {},
        "wallSeconds": round(secs, 2),
        "peakRss": rss,
        "timings": report.get("timings"),
    }
    for c in report["candidates"]:
        agg["statuses"][c["status"]] = agg["statuses"].get(c["status"], 0) + 1
    if public:
        agg["candidateList"] = [{"id": c["id"], "class": c["class"], "status": c["status"],
                                 "entries": [x["selector"] for x in c["entries"]]} for c in report["candidates"]]
        agg["refusedReasons"] = sorted({re.sub(r"`[^`]*`", "X", t[1]) for t in i["tablesRefused"]})
    else:
        agg["refusedReasons"] = sorted({re.sub(r"`[^`]*`", "X", t[1]) for t in i["tablesRefused"]})
    return agg


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--old", required=True)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--private", action="append", default=[])
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    corpus = [("saas-starter", os.path.join(ROOT, ".corpora/saas-starter"), True),
              ("midday", os.path.join(ROOT, ".corpora/midday"), True)]
    corpus += [(os.path.basename(p.rstrip("/")), p, False) for p in args.private]
    rows = {}
    for name, path, public in corpus:
        for phase, binary in (("phase11", args.old), ("phase12", NEW)):
            results = [run(binary, path) for _ in range(args.runs)]
            report = results[0][0]
            secs = statistics.median(r[1] for r in results)
            rss = max(r[2] or 0 for r in results)
            agg = aggregate(report, secs, rss, public)
            agg["project"] = name
            agg["phase"] = phase
            agg["public"] = public
            rows[(name, phase)] = agg
            with open(os.path.join(args.out, f"corpus-{name}-{phase}.json"), "w") as f:
                json.dump(agg, f, indent=1, sort_keys=True)
                f.write("\n")
    # Cache: cold and warm discovery (extraction cache) on midday.
    cache_rows = []
    midday = os.path.join(ROOT, ".corpora/midday")
    for phase, binary in (("phase11", args.old), ("phase12", NEW)):
        cache = os.path.join(args.out, f"midday-cache-{phase}.json")
        if os.path.exists(cache):
            os.remove(cache)
        cold = run(binary, midday, "--cache", cache)
        warm = run(binary, midday, "--cache", cache)
        same = json.dumps(cold[0]["candidates"], sort_keys=True) == json.dumps(warm[0]["candidates"], sort_keys=True)
        cache_rows.append((phase, cold[1], cold[2], warm[1], warm[2], warm[0]["index"]["cacheHits"], same,
                           cold[0]["index"]["entriesExtracted"], cold[0]["index"]["effectfulEntries"]))
        os.remove(cache)
    lines = ["# Phase 12 corpus: Phase 11 binary vs Phase 12", "",
             f"Median of {args.runs} runs (wall), peak RSS the maximum; discovery with bounded model checking; no project executed.", "",
             "| project | phase | modules | entry cand. | extracted | effectful | tables modeled / refused | generated cols | unknown ops | candidates | model-checkable | counterexamples | wall | peak RSS | ms / extracted entry |",
             "| --- | --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for (name, phase), a in rows.items():
        per = a["wallSeconds"] * 1000 / a["entriesExtracted"] if a["entriesExtracted"] else 0
        lines.append(f"| {name} | {phase} | {a['modules']} | {a['entryCandidates']} | {a['entriesExtracted']} | {a['effectfulEntries']} | {a['tablesModeled']} / {a['tablesRefused']} | {a['generatedColumns']} | {a['unknownOperations']} | {a['candidates']} | {a['modelCheckable']} | {a['counterexamples']} | {a['wallSeconds']:.2f} s | {mib(a['peakRss'])} | {per:.2f} |")
    lines += ["", "## Candidate statuses", ""]
    for (name, phase), a in rows.items():
        lines.append(f"* {name} {phase}: {a['statuses'] or '{}'}")
    lines += ["", "## Unknown operations by category (Phase 12)", ""]
    cats = sorted({k for (n, p), a in rows.items() if p == "phase12" for k in a["unknownBreakdown"]})
    names = [n for (n, p) in rows if p == "phase12"]
    lines.append("| category | " + " | ".join(names) + " |")
    lines.append("| --- |" + " ---: |" * len(names))
    for k in cats:
        lines.append(f"| {k} | " + " | ".join(str(rows[(n, 'phase12')]['unknownBreakdown'].get(k, 0)) for n in names) + " |")
    lines += ["", "## midday extraction cache (cold, then warm)", "",
              "| phase | cold wall | cold RSS | warm wall | warm RSS | warm hits | same candidates | extracted | effectful |",
              "| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |"]
    for r in cache_rows:
        lines.append(f"| {r[0]} | {r[1]:.2f} s | {mib(r[2])} | {r[3]:.2f} s | {mib(r[4])} | {r[5]} | {r[6]} | {r[7]} | {r[8]} |")
    with open(os.path.join(args.out, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
