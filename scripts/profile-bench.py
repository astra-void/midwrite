#!/usr/bin/env python3
"""Explorer storage profile: where the explorer's time goes, an earlier
binary against this build, and how long the first counterexample takes.

1. CPU share of state canonicalization (encoding, fragment interning) and of
   the visited-table lookup, from the macOS sampling profiler (`sample`,
   1 ms interval) on the actor-scaling workload of scripts/model-bench.py
   (7 actors, sleep sets). Shares are inclusive sample counts of the named
   functions on the exploring thread; they are profiler estimates, not
   instrumented timings.
2. Counterexample discovery: the same workload without the hypothetical
   unique constraint (a duplicate invitation is reachable), `--no-minimize`,
   median of N runs: time to the first violation and states stored by then.

The symbol patterns cover two storage layouts: `flat` (a std HashMap of
flat canonical encodings, before interned fragment keys) and `interned` (the
current key table and interner). --old-layout says which one the old binary
uses (default flat).

Usage: scripts/profile-bench.py OLD_BINARY OUT.md [N] [--old-layout flat|interned]
       (macOS only)
"""
import argparse
import importlib.util
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
spec = importlib.util.spec_from_file_location("bench", os.path.join(ROOT, "scripts/model-bench.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

# Inclusive buckets on (v0-mangled) symbols: (label, flat-layout pattern,
# interned-layout pattern, ancestors that exclude a frame in the flat
# layout). A frame counts once, at its outermost occurrence on a stack.
BUCKETS = [
    ("canonical encoding", r"5State9canonical$", r"5State6encode$", None),
    ("fragment interning", None, r"8Interner2id$", None),
    ("whole canonical key", r"5State9canonical$", r"5State13canonical_key$", None),
    # The flat layout keeps visited states in a std HashMap<Vec<u8>, u32>:
    # hash-table frames called from the explorer, not from the engine or the
    # encoder.
    ("visited-table lookup / insert", r"9hashbrown", r"8KeyTable6intern$", r"6engine|5State9canonical$"),
    ("transition execution (engine)", r"16midwrite_model6engine", r"16midwrite_model6engine", None),
]
LINE = re.compile(r"^(?P<indent>[ +!:|]*)(?P<count>\d+) (?P<sym>\S+)")


def profile(binary, scenario, seconds, flat):
    proc = subprocess.Popen([binary, "model-check", scenario, "--format", "json"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.0)
    out = tempfile.NamedTemporaryFile(suffix=".txt", delete=False).name
    subprocess.run(["sample", str(proc.pid), str(seconds), "1", "-mayDie", "-file", out],
                   capture_output=True, text=True)
    proc.wait()
    text = open(out).read()
    os.unlink(out)
    graph = text.split("Call graph:", 1)[1].split("Total number in stack", 1)[0]
    # The explorer runs on the main thread (the first one listed).
    main_thread = re.split(r"\n    \d+ Thread_", "\n" + graph.strip("\n"))[1]
    total = int(re.search(r"\n    (\d+) Thread_", "\n" + graph.strip("\n")).group(1))
    found = {b[0]: 0 for b in BUCKETS}
    stack = []  # (depth, symbol, bucket labels open at this frame)
    for line in main_thread.splitlines()[1:]:
        m = LINE.match(line)
        if not m:
            continue
        depth, count, sym = len(m.group("indent")), int(m.group("count")), m.group("sym")
        while stack and stack[-1][0] >= depth:
            stack.pop()
        open_labels = set().union(*(s[2] for s in stack)) if stack else set()
        here = set()
        for label, p_flat, p_interned, exclude in BUCKETS:
            pat = p_flat if flat else p_interned
            if not pat or not re.search(pat, sym):
                continue
            if exclude and flat and any(re.search(exclude, s[1]) for s in stack):
                continue
            here.add(label)
            if label not in open_labels:
                found[label] += count
        stack.append((depth, sym, here))
    return total, found


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("old")
    ap.add_argument("out")
    ap.add_argument("n", nargs="?", type=int, default=3)
    ap.add_argument("--old-layout", choices=["flat", "interned"], default="flat")
    a = ap.parse_args()
    old, out, n, old_layout = a.old, a.out, a.n, a.old_layout
    binaries = (("old", old, old_layout == "flat"), ("new", bench.NEW, False))
    lines = ["# Explorer storage profile", "",
             f"{platform.platform()}; old binary = {os.path.basename(old)} ({old_layout} storage layout); this build = target/release/midwrite.", ""]
    lines += ["## 1. Where exploration time goes (7 actors, sleep sets, `sample` 1 ms for 20 s)", "",
              "| binary | samples on the exploring thread | " + " | ".join(b[0] for b in BUCKETS) + " |",
              "| --- | --- | " + " | ".join("---" for _ in BUCKETS) + " |"]
    path = bench.temp_scenario(bench.team(7))
    for label, binary, flat in binaries:
        total, found = profile(binary, path, 20, flat)
        cells = []
        for b, p_flat, p_interned, _ in BUCKETS:
            pat = p_flat if flat else p_interned
            cells.append("n/a" if pat is None else f"{found[b]} ({100 * found[b] / total:.1f} %)")
        lines.append(f"| {label} | {total} | " + " | ".join(cells) + " |")
        print(lines[-1], flush=True)
    os.unlink(path)

    lines += ["", f"## 2. Counterexample discovery (no unique constraint, `--no-minimize`, median of {n})", "",
              "| actors | binary | verdict | time to first violation ms | exploration wall ms | states stored | peak RSS MiB |",
              "| --- | --- | --- | --- | --- | --- | --- |"]
    for k in (3, 4, 5, 6):
        doc = bench.team(k)
        for t in doc["database"]["tables"]:
            t.pop("unique", None)
        path = bench.temp_scenario(doc)
        for label, binary, _ in binaries:
            runs = [bench.mc(binary, path, "--no-minimize") for _ in range(n)]
            r = runs[0][0]
            first = statistics.median(x[0]["timing"].get("firstViolationMs", -1) for x in runs)
            wall = statistics.median(x[0]["timing"]["wallMs"] for x in runs)
            rss = statistics.median(x[2] for x in runs)
            lines.append(f"| {k} | {label} | {r['exploration']['verdict']} | {first} | {wall} | {r['exploration']['metrics']['states']} | {rss:.1f} |")
            print(lines[-1], flush=True)
        os.unlink(path)
    open(out, "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
