#!/usr/bin/env python3
"""Time a midwrite binary on every project of one or more corpus manifests.

    scripts/bench.py --bin target/release/midwrite --runs 5 \
        --out reports/bench.tsv corpora/manifest.json [-- extra check args]

Each project is scanned with `midwrite check . --config <cfg> --format json
--fail-on never` from the project root, `--runs` times (after one untimed
warm-up run that fills the OS file cache). Every run is a fresh child process
so that peak RSS (getrusage of the child) is per run. Writes one TSV row per
run and keeps the JSON report of the last run as <out>.<project>.json.
Prints median wall time and median peak RSS per project.
"""

import argparse
import json
import os
import resource
import statistics
import subprocess
import sys
import time


def measure(cmd, cwd):
    """Run cmd in a forked helper so RUSAGE_CHILDREN covers only this run."""
    read_fd, write_fd = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(read_fd)
        start = time.perf_counter()
        proc = subprocess.run(cmd, cwd=cwd, capture_output=True)
        wall = time.perf_counter() - start
        rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        if sys.platform != "darwin":
            rss *= 1024  # Linux reports KiB
        payload = json.dumps(
            {"wall": wall, "rss": rss, "code": proc.returncode, "stdout": proc.stdout.decode()}
        ).encode()
        with os.fdopen(write_fd, "wb") as w:
            w.write(payload)
        os._exit(0)
    os.close(write_fd)
    with os.fdopen(read_fd, "rb") as r:
        data = r.read()
    os.waitpid(pid, 0)
    return json.loads(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin", required=True)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--out", required=True)
    parser.add_argument("manifests", nargs="+")
    argv = sys.argv[1:]
    extra = []
    if "--" in argv:
        extra = argv[argv.index("--") + 1 :]
        argv = argv[: argv.index("--")]
    args = parser.parse_args(argv)
    binary = os.path.abspath(args.bin)
    out = os.path.abspath(args.out)
    rows = ["project\trun\twall_s\tpeak_rss_bytes\tdiagnostics\twarnings\tinfo\tfunctions"]
    summary = []
    for manifest_path in args.manifests:
        manifest_path = os.path.abspath(manifest_path)
        base = os.path.dirname(manifest_path)
        with open(manifest_path) as f:
            manifest = json.load(f)
        for project in manifest["projects"]:
            path = project["path"]
            if not os.path.isabs(path):
                path = os.path.normpath(os.path.join(base, path))
            if not os.path.isdir(path):
                print(f"skip {project['name']}: {path} not found", file=sys.stderr)
                continue
            config = os.path.join(base, project["config"])
            cmd = [binary, "check", ".", "--config", config, "--format", "json", "--fail-on", "never", *extra]
            measure(cmd, path)  # warm-up
            walls, rsss = [], []
            report = None
            for run in range(1, args.runs + 1):
                result = measure(cmd, path)
                report = json.loads(result["stdout"])
                s = report["summary"]
                walls.append(result["wall"])
                rsss.append(result["rss"])
                rows.append(
                    "\t".join(
                        str(v)
                        for v in [
                            project["name"],
                            run,
                            f"{result['wall']:.4f}",
                            result["rss"],
                            len(report["diagnostics"]),
                            s["diagnostics"]["warning"],
                            s["diagnostics"]["info"],
                            s["functionsAnalyzed"],
                        ]
                    )
                )
            with open(f"{os.path.splitext(out)[0]}.{project['name']}.json", "w") as f:
                json.dump(report, f, indent=2, sort_keys=True)
                f.write("\n")
            summary.append(
                (
                    project["name"],
                    statistics.median(walls),
                    min(walls),
                    max(walls),
                    statistics.median(rsss),
                    report["summary"]["filesDiscovered"],
                    report["summary"]["functionsAnalyzed"],
                    len(report["diagnostics"]),
                )
            )
    with open(out, "w") as f:
        f.write("\n".join(rows) + "\n")
    print(f"{'project':<20} {'median_s':>9} {'min_s':>8} {'max_s':>8} {'rss_MiB':>8} {'files':>6} {'funcs':>6} {'diags':>6}")
    for name, med, lo, hi, rss, files, funcs, diags in summary:
        print(f"{name:<20} {med:9.4f} {lo:8.4f} {hi:8.4f} {rss / 1048576:8.1f} {files:6} {funcs:6} {diags:6}")


if __name__ == "__main__":
    main()
