#!/usr/bin/env python3
"""Phase 8 source digest: SHA-256 over the sorted (path, SHA-256) list of
every file that is built or tested — crates, runtime, fixtures (without
node_modules and build output), scripts, ci, corpus manifests and
configs, Cargo manifests (not docs or reports). Platform independent: the same tree yields the same
digest on macOS and Linux. Usage: scripts/phase8-digest.py [ROOT]
"""
import hashlib
import os
import sys

ROOT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), ".."))
INCLUDE = ["crates", "runtime", "fixtures", "scripts", "ci", "corpora", "Cargo.toml", "Cargo.lock"]
SKIP_DIRS = {"node_modules", "dist", "target", ".gen", "__pycache__"}


def files():
    for top in INCLUDE:
        path = os.path.join(ROOT, top)
        if os.path.isfile(path):
            yield top
            continue
        for d, dirs, names in os.walk(path):
            dirs[:] = sorted(x for x in dirs if x not in SKIP_DIRS and not x.startswith(".gen-"))
            for n in names:
                if n == ".DS_Store":
                    continue
                yield os.path.relpath(os.path.join(d, n), ROOT)


lines = []
for rel in sorted(files()):
    with open(os.path.join(ROOT, rel), "rb") as f:
        lines.append(f"{hashlib.sha256(f.read()).hexdigest()}  {rel.replace(os.sep, '/')}\n")
print(hashlib.sha256("".join(lines).encode()).hexdigest(), len(lines))
