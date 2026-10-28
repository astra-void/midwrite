#!/usr/bin/env python3
"""Phase 15 corpus coverage: Phase 14 binary vs Phase 15 binary (derived from
scripts/phase14-corpus.py; only the labels changed).

Usage:
  scripts/phase15-corpus.py OUT_DIR --old BIN [--new BIN] [--runs N]
      [--private NAME=DIR ...]

Static discovery with bounded model checking on .corpora/saas-starter and
.corpora/midday (midday twice with each binary: without and with
corpora/contracts/midday.json, which declares its replica factory) and on
every --private DIR (a read-only copy of a private project; only aggregate
counts and generic categories are written: no path, selector or identifier).
No project is executed. Operation tallies (statements by connection, unknown
operations by category) are read from the discovery extraction cache of one
extra run per configuration.

Writes OUT_DIR/<project>-<config>.json and OUT_DIR/SUMMARY.md.
"""
import argparse
import collections
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAIN = os.environ.get("COMMITRIFT_MAIN_CHECKOUT", ROOT)
NEW = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))

# The categories of `commitrift discover` (crates/commitrift-discover/src/pipeline.rs).
RULES = [
    ("read through a read replica (replica modeling not enabled)", "read replica (modeling not enabled)"),
    ("read through a read replica or the primary", "read replica or primary (ambiguous handle)"),
    ("read through a read replica with a row lock", "row lock through a read replica"),
    ("database handle of a replicated database", "replicated database: routing unknown"),
    ("package call with a verified built-in contract", "package call with a contract (result not modeled)"),
    ("method calls are not resolved", "unresolved method call"),
    ("imported from the package", "call into an unanalyzed package"),
    ("is not bound to a table", "table not modeled (schema)"),
    ("database handle of an unresolved type", "database handle of an unresolved type"),
    ("is not a column of table", "column not modeled (schema)"),
    ("raw SQL", "unsupported query (raw SQL)"),
    ("select predicate", "unsupported query shape"),
    ("findFirst option", "unsupported query shape"),
    ("mutation", "unsupported query shape"),
    ("loop over a collection", "loop over a collection of unknown length"),
    ("exceeds the inline depth", "inline depth limit"),
    ("re-export", "re-export not followed"),
    ("no exported function", "unresolved import"),
    ("not a function declared", "unresolved call"),
    ("unresolved identifier", "unresolved identifier"),
    ("transaction", "unsupported transaction shape"),
    ("expression of kind", "unsupported expression"),
    ("optional chain", "unsupported expression"),
    ("operator", "unsupported expression"),
    ("destructuring", "unsupported binding pattern"),
    ("spread", "unsupported expression"),
    ("computed member", "unsupported expression"),
]


def category(detail):
    for needle, cat in RULES:
        if needle in detail:
            return cat
    return "other"


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
    return secs, rss, p.stdout


def tally(cache_path):
    c = json.load(open(cache_path))
    statements = collections.Counter()
    unknown = collections.Counter()
    entries = collections.Counter()
    for v in c["entries"].values():
        if "Ok" not in v:
            continue
        conns = set()
        for code in v["Ok"]["codes"]:
            for s in code:
                op = s["op"]
                if op.get("op") == "db":
                    kind = next(k for k in ("select", "selectCount", "update", "insert", "delete") if k in op)
                    statements[f"{op['conn']} {kind}"] += 1
                    conns.add(op["conn"])
                elif op.get("op") == "begin":
                    statements["transactions (begin)"] += 1
                elif op.get("op") == "unknown":
                    unknown[category(op.get("detail", ""))] += 1
        for k in conns:
            entries[f"entries with {k} statements"] += 1
    return dict(sorted(statements.items())), dict(sorted(unknown.items())), dict(sorted(entries.items()))


def discover(binary, path, extra, runs):
    results = []
    for _ in range(runs):
        secs, rss, out = timed([binary, "discover", path, "--format", "json", "--model-check", *extra])
        results.append((json.loads(out), secs, rss))
    report = results[0][0]
    with tempfile.TemporaryDirectory() as d:
        cache = os.path.join(d, "cache.json")
        subprocess.run([binary, "discover", path, "--format", "json", "--cache", cache, *extra],
                       capture_output=True, text=True)
        statements, unknown_ops, entry_conns = tally(cache)
    i, s = report["index"], report["summary"]
    agg = {
        "entryCandidates": i["entryCandidates"],
        "entriesExtracted": i["entriesExtracted"],
        "effectfulEntries": i["effectfulEntries"],
        "readOnlyEntries": i["readOnlyEntries"],
        "resources": i["resources"],
        "tablesModeled": i["tablesModeled"],
        "unknownOperations": i["unknownOperations"],
        "unknownBreakdown": s.get("unknownOperationBreakdown", {}),
        "candidates": s["candidates"],
        "modelCheckable": s["modelCheckable"],
        "counterexamples": s["modelCounterexamples"],
        "statuses": dict(collections.Counter(c["status"] for c in report["candidates"])),
        "statements": statements,
        "unknownByCategoryInCache": unknown_ops,
        "entryConnections": entry_conns,
        "replicaNotes": [n for n in report.get("notes", []) if "replica" in n],
        "wallSeconds": round(statistics.median(r[1] for r in results), 2),
        "peakRss": max(r[2] or 0 for r in results),
    }
    return agg, report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--old", required=True)
    ap.add_argument("--new", default=NEW)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--private", action="append", default=[], help="NAME=DIR")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    midday_contracts = os.path.join(ROOT, "corpora", "contracts", "midday.json")
    configs = [
        ("saas-starter", os.path.join(MAIN, ".corpora/saas-starter"), True, [
            ("phase14", args.old, []), ("phase15", args.new, [])]),
        ("midday", os.path.join(MAIN, ".corpora/midday"), True, [
            ("phase14", args.old, []), ("phase15", args.new, []),
            ("phase14-declared", args.old, ["--contracts", midday_contracts]),
            ("phase15-declared", args.new, ["--contracts", midday_contracts])]),
    ]
    for spec in args.private:
        name, path = spec.split("=", 1)
        configs.append((name, path, False, [("phase14", args.old, []), ("phase15", args.new, [])]))
    rows = []
    for name, path, public, runs in configs:
        for label, binary, extra in runs:
            agg, report = discover(binary, path, extra, args.runs)
            agg["project"] = name
            agg["config"] = label
            if public:
                agg["candidateList"] = [
                    {"id": c["id"], "class": c["class"], "status": c["status"],
                     "entries": [x["selector"] for x in c["entries"]]} for c in report["candidates"]]
            rows.append(agg)
            with open(os.path.join(args.out, f"{name}-{label}.json"), "w") as f:
                json.dump(agg, f, indent=1, sort_keys=True)
                f.write("\n")
    mib = lambda n: f"{n / 1048576:.0f} MiB" if n else "?"
    lines = [
        "# Phase 15 corpus: Phase 14 binary vs Phase 15 binary", "",
        f"Median of {args.runs} runs (wall), peak RSS the maximum; `commitrift discover --model-check`; no project executed. "
        "`phase14-declared`/`phase15-declared`: with `corpora/contracts/midday.json` (the declared replica factory).", "",
        "| project | config | extracted | effectful | read-only | resources | unknown ops | candidates | model-checkable | counterexamples | statuses | wall | peak RSS |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |",
    ]
    for a in rows:
        lines.append(
            f"| {a['project']} | {a['config']} | {a['entriesExtracted']} | {a['effectfulEntries']} | {a['readOnlyEntries']} | "
            f"{a['resources']} | {a['unknownOperations']} | {a['candidates']} | {a['modelCheckable']} | {a['counterexamples']} | "
            f"{a['statuses'] or '{}'} | {a['wallSeconds']:.2f} s | {mib(a['peakRss'])} |")
    lines += ["", "## Database statements in extracted entries (from the extraction cache)", "",
              "| project | config | statements |", "| --- | --- | --- |"]
    for a in rows:
        lines.append(f"| {a['project']} | {a['config']} | {a['statements'] or '{}'} |")
    cats = sorted({k for a in rows for k in a["unknownBreakdown"]})
    lines += ["", "## Unknown operations by category (discovery report)", "",
              "| category | " + " | ".join(f"{a['project']} {a['config']}" for a in rows) + " |",
              "| --- |" + " ---: |" * len(rows)]
    for k in cats:
        lines.append(f"| {k} | " + " | ".join(str(a["unknownBreakdown"].get(k, 0)) for a in rows) + " |")
    with open(os.path.join(args.out, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
