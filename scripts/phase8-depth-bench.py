#!/usr/bin/env python3
"""Phase 8: static-analysis cost by call depth, Phase 7 binary vs Phase 8.

For each binary (the preserved Phase 7 `midwrite-corpus` and this build)
and each call depth, runs `midwrite-corpus run --timings` REPEAT times
(the two binaries interleaved) on the given manifests and reports, per
project, the median total, summary-construction and path-analysis time,
the expansion/reuse counters, the number of findings (which must be equal
for both binaries) and the median peak RSS of the whole run (macOS
`/usr/bin/time -l`, Linux `/usr/bin/time -v`).

Usage: scripts/phase8-depth-bench.py PHASE7_CORPUS_BINARY OUT.json [REPEAT] [MANIFEST...]
"""
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.path.join(ROOT, "target", "release", "midwrite-corpus")
DEPTHS = [3, 4, 6, 8, 12]


def run(binary, depth, manifests):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "r.json")
        time_flag = "-l" if platform.system() == "Darwin" else "-v"
        cmd = ["/usr/bin/time", time_flag, binary, "run", "--timings", "--output", out,
               "--max-call-depth", str(depth)]
        for m in manifests:
            cmd += ["--manifest", m]
        p = subprocess.run(cmd, capture_output=True, text=True, check=True)
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
                "expansions": t["summaryExpansions"],
                "reuses": t["summaryReuses"],
                "canonicalReuses": t.get("summaryCanonicalReuses", 0),
                "findings": len(pr["findings"]),
                "ids": sorted(f["id"] for f in pr["findings"]),
            }
        return projects, rss


def main():
    old = sys.argv[1]
    out = sys.argv[2]
    repeat = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    manifests = sys.argv[4:] or [os.path.join(ROOT, "corpora", "manifest.json"),
                                 os.path.join(ROOT, "corpora", "manifest.local.json")]
    rows = []
    for depth in DEPTHS:
        samples = {"phase7": [], "phase8": []}
        rss = {"phase7": [], "phase8": []}
        for _ in range(repeat):
            for label, binary in (("phase7", old), ("phase8", NEW)):
                projects, r = run(binary, depth, manifests)
                samples[label].append(projects)
                rss[label].append(r)
        names = samples["phase8"][0].keys()
        for name in names:
            row = {"project": name, "depth": depth}
            for label in ("phase7", "phase8"):
                runs = [s[name] for s in samples[label]]
                for key in ("totalMs", "summariesMs", "analysisMs"):
                    row[f"{label}.{key}"] = round(statistics.median(r[key] for r in runs), 1)
                for key in ("expansions", "reuses", "canonicalReuses", "findings"):
                    row[f"{label}.{key}"] = runs[0][key]
            if samples["phase7"][0][name]["ids"] != samples["phase8"][0][name]["ids"]:
                raise SystemExit(f"{name} depth {depth}: findings differ between the binaries")
            rows.append(row)
        rows.append({"depth": depth, "rssMiB.phase7": round(statistics.median(rss["phase7"]), 1),
                     "rssMiB.phase8": round(statistics.median(rss["phase8"]), 1)})
        print(json.dumps(rows[-len(names) - 1:]), flush=True)
    json.dump({"repeat": repeat, "machine": platform.platform(), "rows": rows},
              open(out, "w"), indent=2)


if __name__ == "__main__":
    main()
