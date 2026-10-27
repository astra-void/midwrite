#!/usr/bin/env python3
"""Phase 10 benchmark workloads: JSON scenarios written to a directory.

Usage: scripts/phase10-workloads.py OUT_DIR
"""
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def team(k, extra_bounds=None):
    base = json.load(open(os.path.join(ROOT, "fixtures/phase7/scenarios/team-app-invite-double-submit.json")))
    doc = json.loads(json.dumps(base))
    doc.pop("sourceVerification", None)
    doc["actors"] = [dict(base["actors"][0], name=f"Submit{i+1}") for i in range(k)]
    doc["bounds"] = {"maxSteps": 250, "maxActors": 8, "maxStates": 50000000, **(extra_bounds or {})}
    doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/phase7/team-app")
    for t in doc["database"]["tables"]:
        if t["name"] == "invitations":
            t["unique"] = [["teamId", "email"]]
    return doc


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    for k in (3, 4, 5):
        json.dump(team(k), open(os.path.join(out, f"team-{k}.json"), "w"), indent=1)
    print("wrote", os.listdir(out))


main()
