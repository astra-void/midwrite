#!/usr/bin/env python3
"""Phase 7: static-analysis cost by call depth, legacy vs compact path walk.

For each path mode (MIDWRITE_PATHS=legacy|compact) and each call depth,
runs `midwrite-corpus run --timings` REPEAT times on the given manifests
and reports the median total and path-analysis time per project, the number
of findings (which must not depend on the mode) and the peak RSS of the
whole run (macOS `/usr/bin/time -l`, Linux `/usr/bin/time -v`).

Usage: scripts/phase7-depth-bench.py OUT.json [REPEAT] [MANIFEST...]
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
CORPUS = os.path.join(ROOT, "target", "release", "midwrite-corpus")


def run(mode, depth, manifests):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "r.json")
        time_flag = "-l" if platform.system() == "Darwin" else "-v"
        cmd = ["/usr/bin/time", time_flag, CORPUS, "run", "--timings", "--output", out,
               "--max-call-depth", str(depth)]
        for m in manifests:
            cmd += ["--manifest", m]
        env = dict(os.environ, MIDWRITE_PATHS=mode)
        p = subprocess.run(cmd, env=env, capture_output=True, text=True, check=True)
        rss = None
        m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
        if m:
            rss = int(m.group(1)) / (1024 * 1024)  # bytes on macOS
        m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
        if m:
            rss = int(m.group(1)) / 1024
        report = json.load(open(out))
        projects = {}
        for pr in report["projects"]:
            projects[pr["name"]] = {
                "totalMs": pr["timings"]["totalMs"],
                "analysisMs": pr["timings"]["analysisMs"],
                "summariesMs": pr["timings"]["summariesMs"],
                "findings": len(pr["findings"]),
            }
        return projects, rss


def main():
    out = sys.argv[1]
    repeat = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    manifests = sys.argv[3:] or [os.path.join(ROOT, "corpora", "manifest.json"),
                                 os.path.join(ROOT, "corpora", "manifest.local.json")]
    rows = []
    for depth in [3, 4, 5, 6, 8]:
        for mode in ["legacy", "compact"]:
            samples = [run(mode, depth, manifests) for _ in range(repeat)]
            names = samples[0][0].keys()
            for name in names:
                findings = {s[0][name]["findings"] for s in samples}
                rows.append({
                    "depth": depth,
                    "mode": mode,
                    "project": name,
                    "findings": sorted(findings),
                    "totalMs": statistics.median(s[0][name]["totalMs"] for s in samples),
                    "analysisMs": statistics.median(s[0][name]["analysisMs"] for s in samples),
                    "summariesMs": statistics.median(s[0][name]["summariesMs"] for s in samples),
                })
            rows.append({
                "depth": depth,
                "mode": mode,
                "project": "(whole run)",
                "peakRssMiB": statistics.median(s[1] for s in samples if s[1] is not None),
            })
            print(f"depth {depth} {mode} done", file=sys.stderr)
    json.dump({"repeat": repeat, "machine": platform.platform(), "rows": rows}, open(out, "w"), indent=1)
    by = {}
    for r in rows:
        by.setdefault((r["project"], r["depth"]), {})[r["mode"]] = r
    print("| project | depth | findings | legacy total / analysis ms | compact total / analysis ms | analysis change |")
    print("| --- | --- | --- | --- | --- | --- |")
    for (project, depth), modes in sorted(by.items()):
        if project == "(whole run)":
            continue
        l, c = modes["legacy"], modes["compact"]
        assert l["findings"] == c["findings"], (project, depth)
        change = (c["analysisMs"] - l["analysisMs"]) / l["analysisMs"] * 100 if l["analysisMs"] else 0
        print(f"| {project} | {depth} | {l['findings'][0]} | {l['totalMs']:.1f} / {l['analysisMs']:.1f} | {c['totalMs']:.1f} / {c['analysisMs']:.1f} | {change:+.0f} % |")
    print()
    print("| depth | legacy peak RSS MiB | compact peak RSS MiB |")
    print("| --- | --- | --- |")
    for depth in [3, 4, 5, 6, 8]:
        l = by[("(whole run)", depth)]["legacy"]["peakRssMiB"]
        c = by[("(whole run)", depth)]["compact"]["peakRssMiB"]
        print(f"| {depth} | {l:.1f} | {c:.1f} |")


if __name__ == "__main__":
    main()
