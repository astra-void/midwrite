#!/usr/bin/env python3
"""Attribute corpus diagnostic changes to Phase 5 analysis mechanisms.

    scripts/phase5-attribute.py BEFORE.json AFTER.json IRDIR [--show-private]

BEFORE/AFTER are `midwrite-corpus run` reports; IRDIR holds
`<project>-4.txt` and `<project>-5.txt`, the text IR (`midwrite ir .`) of
each project produced by the Phase 4 and Phase 5 binaries with the same
configuration. For every finding added or removed (by fingerprint), the
function's IR is compared between the two versions and the Phase 5
mechanisms that appear only in the new IR are listed:

  concurrent-call   a resolved call inside Promise.all, now summarized
  array-callback    xs.map/forEach(fn) modeled as a loop calling fn
  method-dispatch   a call resolved by dispatch on a receiver's class
  inert-call        a call resolved because the callee is proven inert
  immediate-call    an immediately invoked function expression
  callback          a parameter call linked to the caller's function
  argument-effect   an unknown call in an argument kept because the
                    resolved callee awaits that parameter
  return-facts      callee return facts on the caller's binding
  path-budget       the function exceeds --max-paths in Phase 5

The attribution is a starting point for manual review, not a verdict.
Private projects (origin != external) print only category counts unless
--show-private is given (never commit that output).
"""

import collections
import json
import re
import sys

MARKERS = [
    ("array-callback", re.compile(r"loop array callback")),
    ("method-dispatch", re.compile(r"\[method of ")),
    ("inert-call", re.compile(r"proven inert:")),
    ("immediate-call", re.compile(r"\[immediately invoked\]")),
    ("callback", re.compile(r"\[callback passed for parameter")),
    ("concurrent-call", re.compile(r"^\s*call#(\d+|N) .* (inlines|proven inert:) .*\(Concurrent\)")),
    ("return-facts", re.compile(r"return \[[^\]]*#\d+[^\]]*\]")),
]
ID = re.compile(r"(effect|call|tx)#\d+")


def functions(path):
    """{(file, function): [lines]} of a text IR dump."""
    out = {}
    file = None
    current = None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.rstrip("\n")
            if line.startswith("file "):
                file = line[5:]
                current = None
                continue
            match = re.match(r"^  (?:async )?fn (.+) @ \d+:\d+$", line)
            if match:
                current = (file, match.group(1))
                out[current] = []
                continue
            if current:
                out[current].append(line)
    return out


def normalized(lines):
    return {ID.sub(r"\1#N", line) for line in lines}


def mechanisms(before, after, finding):
    new_lines = normalized(after) - normalized(before)
    found = []
    for name, pattern in MARKERS:
        if any(pattern.search(line) for line in new_lines):
            found.append(name)
    # Unknown effects in the new IR that are concurrent and were not unknown
    # before at all: arguments kept because the callee awaits them.
    old_unknown = {re.sub(r" @ .*", "", l) for l in normalized(before) if "unknown effect" in l}
    for line in new_lines:
        if "unknown effect" in line and "(Concurrent)" in line:
            if re.sub(r" @ .*", "", line) not in old_unknown and "per element" not in line:
                found.append("argument-effect")
                break
    if "execution paths" in finding["message"]:
        found.append("path-budget")
    return found or ["(none detected)"]


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    show_private = "--show-private" in sys.argv
    before, after, irdir = args
    reports = [json.load(open(before)), json.load(open(after))]
    projects = {}
    for index, report in enumerate(reports):
        for project in report["projects"]:
            entry = projects.setdefault(project["name"], {"origin": project["origin"]})
            entry[index] = {f["id"]: f for f in project["findings"]}
    totals = collections.Counter()
    for name, entry in projects.items():
        old, new = entry.get(0, {}), entry.get(1, {})
        added = [new[i] for i in sorted(set(new) - set(old))]
        removed = [old[i] for i in sorted(set(old) - set(new))]
        if not added and not removed:
            continue
        ir4 = functions(f"{irdir}/{name}-4.txt")
        ir5 = functions(f"{irdir}/{name}-5.txt")
        counts = collections.Counter()
        print(f"## {name}: {len(added)} added, {len(removed)} removed")
        for sign, findings in (("+", added), ("-", removed)):
            for finding in findings:
                key = (finding["file"], finding["function"])
                found = mechanisms(ir4.get(key, []), ir5.get(key, []), finding)
                for mechanism in found:
                    counts[(sign, finding["code"], mechanism)] += 1
                    totals[(sign, mechanism)] += 1
                if entry["origin"] == "external" or show_private:
                    print(
                        f"{sign} {finding['code']} {finding['file']}:{finding['line']} "
                        f"{finding['function']} [{', '.join(found)}]"
                    )
        for (sign, code, mechanism), count in sorted(counts.items()):
            print(f"   {sign} {code} {mechanism}: {count}")
        print()
    print("## totals by mechanism")
    for (sign, mechanism), count in sorted(totals.items()):
        print(f"   {sign} {mechanism}: {count}")


if __name__ == "__main__":
    main()
