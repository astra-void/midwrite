#!/usr/bin/env python3
"""Phase 16: TP/FP/FN/TN of the Phase 12 and Phase 13 sets from their
scorers' per-case output (those scorers print stages, not a confusion
matrix). Rule, as in docs/phase15/PRECISION-RECALL.md §3: a case is predicted
positive when it has a validated model counterexample (`counterexample` and
`validated`); a paired case (`pairedWith`) uses `pairCounterexample`.
Unsupported labels are not counted.

Usage: scripts/phase16-regression-tally.py REPORT.json [...]
"""
import json
import sys

for path in sys.argv[1:]:
    r = json.load(open(path))
    t = {"TP": 0, "FP": 0, "FN": 0, "TN": 0}
    changed = []
    for c in r["cases"]:
        if c["label"] not in ("positive", "negative"):
            continue
        if c.get("pairedWith"):
            pred = bool(c.get("pairCounterexample"))
        else:
            pred = bool(c.get("counterexample")) and c.get("validated") is True
        pos = c["label"] == "positive"
        t[("T" if pred == pos else "F") + ("P" if pred else "N")] += 1
    print(path, t)
