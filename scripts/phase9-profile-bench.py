#!/usr/bin/env python3
"""Phase 9 storage profile: where the explorer's time goes, Phase 8 against
this build, and how long the first counterexample takes.

1. CPU share of state canonicalization (encoding, fragment interning) and of
   the visited-table lookup, from the macOS sampling profiler (`sample`,
   1 ms interval) on the actor-scaling workload of
   scripts/phase9-model-bench.py (7 actors, sleep sets). Shares are
   inclusive sample counts of the named functions on the exploring thread;
   they are profiler estimates, not instrumented timings.
2. Counterexample discovery: the same workload without the hypothetical
   unique constraint (a duplicate invitation is reachable), `--no-minimize`,
   median of N runs: time to the first violation and states stored by then.

Usage: scripts/phase9-profile-bench.py PHASE8_BINARY OUT.md [N]   (macOS only)
"""
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
spec = importlib.util.spec_from_file_location("bench", os.path.join(ROOT, "scripts/phase9-model-bench.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

# Inclusive buckets on (v0-mangled) symbols: (label, Phase 8 pattern,
# Phase 9 pattern, ancestors that exclude a frame). A frame counts once,
# at its outermost occurrence on a stack.
BUCKETS = [
    ("canonical encoding", r"5State9canonical$", r"5State6encode$", None),
    ("fragment interning", None, r"8Interner2id$", None),
    ("whole canonical key", r"5State9canonical$", r"5State13canonical_key$", None),
    # Phase 8 keeps visited states in a std HashMap<Vec<u8>, u32>: hash-table
    # frames called from the explorer, not from the engine or the encoder.
    ("visited-table lookup / insert", r"9hashbrown", r"8KeyTable6intern$", r"6engine|5State9canonical$"),
    ("transition execution (engine)", r"16midwrite_model6engine", r"16midwrite_model6engine", None),
]
LINE = re.compile(r"^(?P<indent>[ +!:|]*)(?P<count>\d+) (?P<sym>\S+)")


def profile(binary, scenario, seconds, phase8):
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
        for label, p8, p9, exclude in BUCKETS:
            pat = p8 if phase8 else p9
            if not pat or not re.search(pat, sym):
                continue
            if exclude and phase8 and any(re.search(exclude, s[1]) for s in stack):
                continue
            here.add(label)
            if label not in open_labels:
                found[label] += count
        stack.append((depth, sym, here))
    return total, found


def main():
    phase8, out = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    lines = ["# Phase 9 storage profile", "",
             f"{platform.platform()}; Phase 8 binary = {os.path.basename(phase8)} (saved release build of 4dc01af); this build = target/release/midwrite.", ""]
    lines += ["## 1. Where exploration time goes (7 actors, sleep sets, `sample` 1 ms for 20 s)", "",
              "| binary | samples on the exploring thread | " + " | ".join(b[0] for b in BUCKETS) + " |",
              "| --- | --- | " + " | ".join("---" for _ in BUCKETS) + " |"]
    path = bench.temp_scenario(bench.team(7))
    for label, binary in (("Phase 8", phase8), ("Phase 9", bench.NEW)):
        total, found = profile(binary, path, 20, label == "Phase 8")
        cells = []
        for b, p8, p9, _ in BUCKETS:
            pat = p8 if label == "Phase 8" else p9
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
        for label, binary in (("Phase 8", phase8), ("Phase 9", bench.NEW)):
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
