#!/usr/bin/env python3
"""Static-analysis cost by call depth (`midwrite-corpus run --timings`).

Usage:
  scripts/depth-bench.py OUT.json [--old CORPUS_BINARY] [--modes MODE ...]
      [--depths N ...] [--repeat N] [MANIFEST ...]

Compares variants of this build's `midwrite-corpus` (target/release):
  --old BIN     the preserved `midwrite-corpus` of an earlier build against
                this build (default depths 3 4 6 8 12);
  --modes ...   path-analysis modes of this build (MIDWRITE_PATHS=legacy,
                compact or shared; default when --old is absent: legacy
                compact, depths 3 4 5 6 8).
For each variant and depth, runs REPEAT times (default 5; the variants
interleaved) on the given manifests (default corpora/manifest.json and
corpora/manifest.local.json) and reports, per project, the median total,
summary-construction and path-analysis time, the expansion/reuse counters,
the number of findings (which must be equal for every variant) and the
median peak RSS of the whole run (macOS `/usr/bin/time -l`, Linux
`/usr/bin/time -v`). Writes OUT.json and prints a Markdown table.
"""
import argparse
import json
import os
import platform
import re
import statistics
import subprocess
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.path.join(ROOT, "target", "release", "midwrite-corpus")


def run(binary, env, depth, manifests):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "r.json")
        time_flag = "-l" if platform.system() == "Darwin" else "-v"
        cmd = ["/usr/bin/time", time_flag, binary, "run", "--timings", "--output", out,
               "--max-call-depth", str(depth)]
        for m in manifests:
            cmd += ["--manifest", m]
        p = subprocess.run(cmd, env=dict(os.environ, **env), capture_output=True, text=True, check=True)
        rss = None
        m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
        if m:
            rss = int(m.group(1)) / (1024 * 1024)
        m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
        if m:
            rss = int(m.group(1)) / 1024
        report = json.load(open(out))
        projects = {}
        for pr in report["projects"]:
            t = pr["timings"]
            projects[pr["name"]] = {
                "totalMs": t["totalMs"],
                "summariesMs": t["summariesMs"],
                "analysisMs": t["analysisMs"],
                "expansions": t.get("summaryExpansions"),
                "reuses": t.get("summaryReuses"),
                "canonicalReuses": t.get("summaryCanonicalReuses", 0),
                "findings": len(pr["findings"]),
                "ids": sorted(f["id"] for f in pr["findings"]),
            }
        return projects, rss


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out")
    ap.add_argument("manifests", nargs="*")
    ap.add_argument("--old", help="an earlier midwrite-corpus binary")
    ap.add_argument("--modes", nargs="+", choices=["legacy", "compact", "shared"])
    ap.add_argument("--depths", nargs="+", type=int)
    ap.add_argument("--repeat", type=int, default=5)
    a = ap.parse_args()
    manifests = a.manifests or [os.path.join(ROOT, "corpora", "manifest.json"),
                                os.path.join(ROOT, "corpora", "manifest.local.json")]
    if a.old:
        variants = [("old", a.old, {}), ("new", NEW, {})]
        variants = [(f"{label} {m}", b, {"MIDWRITE_PATHS": m}) for label, b, _ in variants for m in a.modes] \
            if a.modes else variants
        depths = a.depths or [3, 4, 6, 8, 12]
    else:
        variants = [(m, NEW, {"MIDWRITE_PATHS": m}) for m in (a.modes or ["legacy", "compact"])]
        depths = a.depths or [3, 4, 5, 6, 8]
    labels = [v[0] for v in variants]
    rows = []
    for depth in depths:
        samples = {label: [] for label in labels}
        rss = {label: [] for label in labels}
        for _ in range(a.repeat):
            for label, binary, env in variants:
                projects, r = run(binary, env, depth, manifests)
                samples[label].append(projects)
                rss[label].append(r)
        names = samples[labels[-1]][0].keys()
        for name in names:
            row = {"project": name, "depth": depth}
            for label in labels:
                runs = [s[name] for s in samples[label]]
                for key in ("totalMs", "summariesMs", "analysisMs"):
                    row[f"{label}.{key}"] = round(statistics.median(r[key] for r in runs), 1)
                for key in ("expansions", "reuses", "canonicalReuses", "findings"):
                    row[f"{label}.{key}"] = runs[0][key]
            ids = {label: samples[label][0][name]["ids"] for label in labels}
            if len({json.dumps(v) for v in ids.values()}) != 1:
                raise SystemExit(f"{name} depth {depth}: findings differ between {labels}")
            rows.append(row)
        rows.append({"depth": depth, **{f"rssMiB.{label}": round(statistics.median(x for x in rss[label] if x is not None), 1)
                                         for label in labels}})
        print(f"depth {depth} done", flush=True)
    json.dump({"repeat": a.repeat, "machine": platform.platform(), "variants": labels, "rows": rows},
              open(a.out, "w"), indent=2)
    print("| project | depth | findings | " + " | ".join(f"{label} total / analysis ms" for label in labels) + " |")
    print("| --- | --- | --- | " + " | ".join("---" for _ in labels) + " |")
    for r in rows:
        if "project" not in r:
            continue
        print(f"| {r['project']} | {r['depth']} | {r[labels[0] + '.findings']} | "
              + " | ".join(f"{r[label + '.totalMs']:.1f} / {r[label + '.analysisMs']:.1f}" for label in labels) + " |")
    print()
    print("| depth | " + " | ".join(f"{label} peak RSS MiB" for label in labels) + " |")
    print("| --- | " + " | ".join("---" for _ in labels) + " |")
    for r in rows:
        if "project" not in r:
            print(f"| {r['depth']} | " + " | ".join(f"{r['rssMiB.' + label]:.1f}" for label in labels) + " |")


if __name__ == "__main__":
    main()
