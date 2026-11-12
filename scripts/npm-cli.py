#!/usr/bin/env python3
"""Assemble the npm packages of the `midwrite` CLI.

    python3 scripts/npm-cli.py --out target/npm \
        --binary aarch64-apple-darwin=path/to/midwrite \
        --binary x86_64-unknown-linux-musl=path/to/midwrite ...

Writes target/npm/midwrite (the launcher, from npm/midwrite) and one
target/npm/cli-<os>-<cpu> package per --binary. The version comes from the
workspace Cargo.toml and must match npm/midwrite/package.json, including its
optionalDependencies. Every platform the launcher lists must be given.
Publishes nothing: run `npm publish` in each directory, platform packages
first.
"""
import argparse
import json
import re
import shutil
import stat
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Rust target triple prefix -> (npm os, npm cpu).
TARGETS = {
    "aarch64-apple-darwin": ("darwin", "arm64"),
    "x86_64-apple-darwin": ("darwin", "x64"),
    "aarch64-unknown-linux-musl": ("linux", "arm64"),
    "x86_64-unknown-linux-musl": ("linux", "x64"),
    "aarch64-unknown-linux-gnu": ("linux", "arm64"),
    "x86_64-unknown-linux-gnu": ("linux", "x64"),
    "x86_64-pc-windows-gnu": ("win32", "x64"),
    "x86_64-pc-windows-msvc": ("win32", "x64"),
}


def workspace_version():
    text = (REPO / "Cargo.toml").read_text()
    section = text.split("[workspace.package]", 1)[1]
    return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M).group(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, default=REPO / "target" / "npm")
    parser.add_argument("--binary", action="append", default=[],
                        metavar="TRIPLE=PATH", help="a built midwrite executable")
    args = parser.parse_args()

    version = workspace_version()
    launcher = json.loads((REPO / "npm/midwrite/package.json").read_text())
    if launcher["version"] != version:
        sys.exit(f"npm/midwrite/package.json is {launcher['version']}, Cargo.toml is {version}")
    wanted = launcher["optionalDependencies"]
    for name, pinned in wanted.items():
        if pinned != version:
            sys.exit(f"optionalDependencies pins {name}@{pinned}, expected {version}")

    out = args.out.resolve()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    licenses = [REPO / "LICENSE-MIT", REPO / "LICENSE-APACHE"]
    built = set()
    for spec in args.binary:
        triple, _, path = spec.partition("=")
        if triple not in TARGETS or not path:
            sys.exit(f"--binary {spec!r}: expected one of {', '.join(TARGETS)}=PATH")
        os_name, cpu = TARGETS[triple]
        name = f"@midwrite/cli-{os_name}-{cpu}"
        if name not in wanted:
            sys.exit(f"{name} is not listed in the launcher's optionalDependencies")
        if name in built:
            sys.exit(f"{name} given twice")
        built.add(name)
        directory = out / f"cli-{os_name}-{cpu}"
        (directory / "bin").mkdir(parents=True)
        exe = "midwrite.exe" if os_name == "win32" else "midwrite"
        binary = directory / "bin" / exe
        shutil.copy2(path, binary)
        binary.chmod(binary.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        for license_file in licenses:
            shutil.copy2(license_file, directory)
        manifest = {
            "name": name,
            "version": version,
            "description": f"The midwrite binary for {os_name} {cpu} ({triple}). Installed by the `midwrite` package.",
            "homepage": launcher["homepage"],
            "repository": launcher["repository"],
            "license": launcher["license"],
            "os": [os_name],
            "cpu": [cpu],
            "files": [f"bin/{exe}", "README.md", "LICENSE-MIT", "LICENSE-APACHE"],
            "preferUnplugged": True,
            "publishConfig": {"access": "public"},
        }
        (directory / "package.json").write_text(json.dumps(manifest, indent=2) + "\n")
        (directory / "README.md").write_text(
            f"# {name}\n\nThe `midwrite` {version} binary for {os_name} {cpu}, built for "
            f"`{triple}`.{' Static analysis only: built without the replay feature.' if os_name == 'win32' else ''}\n\nDo not install this directly: install "
            "[`midwrite`](https://www.npmjs.com/package/midwrite), which selects "
            "the right platform package.\n"
        )
        print(f"{name}@{version}: {directory}")

    missing = sorted(set(wanted) - built)
    if missing:
        sys.exit(f"no --binary given for {', '.join(missing)}")

    directory = out / "midwrite"
    shutil.copytree(REPO / "npm/midwrite", directory)
    for license_file in licenses:
        shutil.copy2(license_file, directory)
    print(f"midwrite@{version}: {directory}")


if __name__ == "__main__":
    main()
