#!/usr/bin/env python3
"""Compare Phase 1 CLI reports (reports/phase2/baseline/bench-phase1.<project>.json)
with a Phase 2 corpus report by (file, line, code).

    scripts/compare-phase1.py reports/phase2/corpus-phase2-strict.json
"""
import json, os, sys
from collections import Counter

here = os.path.dirname(os.path.abspath(__file__))
base = os.path.join(os.path.dirname(here), "reports", "phase2", "baseline")
report = json.load(open(sys.argv[1]))
verbose = "-v" in sys.argv
for project in report["projects"]:
    path = os.path.join(base, f"bench-phase1.{project['name']}.json")
    if not os.path.exists(path):
        continue
    old = json.load(open(path))["diagnostics"]
    old_keys = Counter((d["location"]["file"], d["location"]["line"], d["code"]) for d in old)
    new_keys = Counter((f["file"], f["line"], f["code"]) for f in project["findings"])
    kept = old_keys & new_keys
    removed = old_keys - new_keys
    added = new_keys - old_keys
    def by_code(c):
        out = Counter()
        for (_, _, code), n in c.items():
            out[code] += n
        return dict(sorted(out.items()))
    print(f"{project['name']}: phase1={sum(old_keys.values())} phase2={sum(new_keys.values())} "
          f"kept={sum(kept.values())} removed={by_code(removed)} added={by_code(added)}")
    if verbose:
        for key in sorted(removed):
            print("   -", *key)
