#!/usr/bin/env python3
"""Real-world corpus coverage and performance: `midwrite discover
--model-check` on the pinned public corpus (and private projects), with this
build and optionally an earlier binary.

Usage:
  scripts/corpus.py [OUT_DIR] [--old BIN] [--new BIN] [--runs N]
      [--private NAME=DIR ...] [--cache-check]

OUT_DIR defaults to reports/corpus. Static discovery with bounded model
checking on .corpora/saas-starter and .corpora/midday (midday twice with
each binary: without and with corpora/contracts/midday.json, which declares
its replica factory) and on every --private DIR (a read-only copy of a
private project; only aggregate counts and generic categories are written:
no path, selector or identifier). No project is executed. Configurations are
labeled `new` (this build, --new, default MIDWRITE_BIN or
target/release/midwrite) and `old` (--old), with `-declared` for the midday
contracts run. Corpus checkouts are looked up in MIDWRITE_MAIN_CHECKOUT
(default: this checkout), so a worktree can use the main checkout's
.corpora/.

Recorded per configuration: median wall time of N runs, peak RSS (maximum),
extraction and model-checking counts, generated columns, candidate statuses,
and every candidate's stage (status / property origin / generic category of
its first blocker; public projects also keep the blocker text, private ones
only the category). Operation tallies (statements by connection, unknown
operations by category) are read from the discovery extraction cache of one
extra run per configuration.

--cache-check also measures discovery on midday with a cold and then a warm
extraction cache (`--cache`) for each binary, and whether both give the same
candidates.

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
MAIN = os.environ.get("MIDWRITE_MAIN_CHECKOUT", ROOT)
NEW = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))

# The categories of `midwrite discover` (crates/midwrite-discover/src/pipeline.rs).
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
    # Later categories, after the earlier ones (first match wins).
    ("refinement", "zod refinement callback"),
    ("superRefine", "zod refinement callback"),
    ("zod", "zod construct outside the modeled subset"),
    ("caught error", "caught error property not modeled"),
    ("loop", "loop outside the bounded subset"),
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
        "generatedColumns": len(i.get("generatedColumns", [])),
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


def origin(c):
    props = c.get("properties") or []
    return ",".join(sorted({p.get("origin", "?") for p in props})) or "none"


def blocker(c):
    """(text, generic category) of the candidate's first blocker."""
    for u in c.get("unsupported") or []:
        text = u if isinstance(u, str) else (u.get("detail") or u.get("reason") or u.get("text") or json.dumps(u, sort_keys=True))
        return text, category(text)
    if c["status"] in ("BLOCKED", "INCONCLUSIVE"):
        return c.get("nextStep") or "", c["status"].lower()
    return "", "none"


def cache_check(out, binaries):
    """Cold, then warm, discovery with the extraction cache on midday."""
    midday = os.path.join(MAIN, ".corpora/midday")
    rows = []
    if not os.path.isdir(midday):
        return rows
    for label, binary in binaries:
        cache = os.path.join(out, f"midday-cache-{label}.json")
        if os.path.exists(cache):
            os.remove(cache)
        cold = timed([binary, "discover", midday, "--format", "json", "--model-check", "--cache", cache])
        warm = timed([binary, "discover", midday, "--format", "json", "--model-check", "--cache", cache])
        os.remove(cache)
        c, w = json.loads(cold[2]), json.loads(warm[2])
        same = json.dumps(c["candidates"], sort_keys=True) == json.dumps(w["candidates"], sort_keys=True)
        rows.append((label, cold[0], cold[1], warm[0], warm[1], w["index"].get("cacheHits"), same,
                     c["index"]["entriesExtracted"], c["index"]["effectfulEntries"]))
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out", nargs="?", default=os.path.join(ROOT, "reports", "corpus"))
    ap.add_argument("--old", help="an earlier midwrite binary to compare with")
    ap.add_argument("--new", default=NEW)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--private", action="append", default=[], help="NAME=DIR")
    ap.add_argument("--cache-check", action="store_true", help="cold vs warm extraction cache on midday")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    midday_contracts = os.path.join(ROOT, "corpora", "contracts", "midday.json")
    binaries = ([("old", args.old)] if args.old else []) + [("new", args.new)]
    configs = [
        ("saas-starter", os.path.join(MAIN, ".corpora/saas-starter"), True,
         [(label, binary, []) for label, binary in binaries]),
        ("midday", os.path.join(MAIN, ".corpora/midday"), True,
         [(label, binary, []) for label, binary in binaries]
         + [(f"{label}-declared", binary, ["--contracts", midday_contracts]) for label, binary in binaries]),
    ]
    for spec in args.private:
        name, path = spec.split("=", 1)
        configs.append((name, path, False, [(label, binary, []) for label, binary in binaries]))
    rows = []
    for name, path, public, runs in configs:
        if not os.path.isdir(path):
            print(f"{name}: {path} not available (skipped)", file=sys.stderr)
            continue
        for label, binary, extra in runs:
            agg, report = discover(binary, path, extra, args.runs)
            agg["project"] = name
            agg["config"] = label
            agg["stages"] = dict(collections.Counter(
                f"{c['status']} / {origin(c)} / {blocker(c)[1]}" for c in report["candidates"]))
            if public:
                agg["candidateList"] = [
                    {"id": c["id"], "class": c["class"], "status": c["status"], "origin": origin(c),
                     "firstBlocker": blocker(c)[0], "nextStep": c.get("nextStep"),
                     "entries": [x["selector"] for x in c["entries"]]} for c in report["candidates"]]
            rows.append(agg)
            with open(os.path.join(args.out, f"{name}-{label}.json"), "w") as f:
                json.dump(agg, f, indent=1, sort_keys=True)
                f.write("\n")
    mib = lambda n: f"{n / 1048576:.0f} MiB" if n else "?"
    title = "old binary vs new binary" if args.old else "this build"
    lines = [
        f"# Corpus coverage: {title}", "",
        f"Median of {args.runs} runs (wall), peak RSS the maximum; `midwrite discover --model-check`; no project executed. "
        "`*-declared`: with `corpora/contracts/midday.json` (the declared replica factory).", "",
        "| project | config | extracted | effectful | read-only | resources | generated cols | unknown ops | candidates | model-checkable | counterexamples | statuses | wall | peak RSS | ms / extracted entry |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: |",
    ]
    for a in rows:
        per = a["wallSeconds"] * 1000 / a["entriesExtracted"] if a["entriesExtracted"] else 0
        lines.append(
            f"| {a['project']} | {a['config']} | {a['entriesExtracted']} | {a['effectfulEntries']} | {a['readOnlyEntries']} | "
            f"{a['resources']} | {a['generatedColumns']} | {a['unknownOperations']} | {a['candidates']} | {a['modelCheckable']} | {a['counterexamples']} | "
            f"{a['statuses'] or '{}'} | {a['wallSeconds']:.2f} s | {mib(a['peakRss'])} | {per:.2f} |")
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
    lines += ["", "## Candidate stages (status / property origin / first-blocker category)", "",
              "| project | config | stages |", "| --- | --- | --- |"]
    for a in rows:
        lines.append(f"| {a['project']} | {a['config']} | {a['stages'] or '{}'} |")
    if args.cache_check:
        lines += ["", "## midday extraction cache (cold, then warm)", "",
                  "| binary | cold wall | cold RSS | warm wall | warm RSS | warm hits | same candidates | extracted | effectful |",
                  "| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |"]
        for r in cache_check(args.out, binaries):
            lines.append(f"| {r[0]} | {r[1]:.2f} s | {mib(r[2])} | {r[3]:.2f} s | {mib(r[4])} | {r[5]} | {r[6]} | {r[7]} | {r[8]} |")
    with open(os.path.join(args.out, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
