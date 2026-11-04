#!/usr/bin/env python3
"""Print findings of a corpus report with source context, for manual review.

    scripts/show-finding.py reports/phase2/corpus-phase2.json midday MW1001 [--start N] [--count N]

Shows each finding's message, its primary location with surrounding lines,
and (from a fresh `midwrite check --format json` of the project) the
related operations with one line of context each. Nothing is labeled.
"""

import json
import os
import subprocess
import sys

report_path, project_name, code = sys.argv[1:4]
start = int(sys.argv[sys.argv.index("--start") + 1]) if "--start" in sys.argv else 0
count = int(sys.argv[sys.argv.index("--count") + 1]) if "--count" in sys.argv else 10
before = int(sys.argv[sys.argv.index("--before") + 1]) if "--before" in sys.argv else 12

here = os.path.dirname(os.path.abspath(__file__))
root = os.path.dirname(here)
report = json.load(open(report_path))
project = next(p for p in report["projects"] if p["name"] == project_name)
manifests = [os.path.join(root, "corpora", m) for m in ("manifest.json", "manifest.local.json")]
entry = None
for m in manifests:
    for p in json.load(open(m))["projects"]:
        if p["name"] == project_name:
            entry = (os.path.dirname(m), p)
base, p = entry
project_root = p["path"] if os.path.isabs(p["path"]) else os.path.normpath(os.path.join(base, p["path"]))
config = os.path.join(base, p["config"])
binary = os.path.join(root, "target", "release", "midwrite")
full = json.loads(
    subprocess.run(
        [binary, "check", ".", "--config", config, "--format", "json", "--fail-on", "never",
         "--policy", report["policy"]],
        cwd=project_root, capture_output=True, text=True,
    ).stdout
)
by_id = {d["fingerprint"]: d for d in full["diagnostics"]}


def lines(path, first, last):
    try:
        text = open(os.path.join(project_root, path)).read().split("\n")
    except OSError:
        return []
    return [(n, text[n - 1]) for n in range(max(1, first), min(len(text), last) + 1)]


findings = [f for f in project["findings"] if f["code"] == code]
for f in findings[start:start + count]:
    print("=" * 100)
    print(f"{f['id']} {f['code']} {f['severity']} {f['confidence']} {f['file']}:{f['line']} in {f['function']}")
    print(f"  {f['message']}")
    for n, text in lines(f["file"], f["line"] - before, f["line"] + 3):
        print(f"  {'>' if n == f['line'] else ' '}{n:5} {text}")
    d = by_id.get(f["id"])
    if d:
        for e in d["effects"]:
            loc = e["location"]
            src = lines(loc["file"], loc["line"], loc["line"])
            text = src[0][1].strip() if src else ""
            print(f"    [{e['role']}] {loc['file']}:{loc['line']}  {text[:110]}")
        for c in d["pathConditions"]:
            print(f"    cond: {c['description']}")
        for l in d["limitations"][:-1]:
            print(f"    note: {l[:200]}")
