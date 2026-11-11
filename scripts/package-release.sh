#!/usr/bin/env bash
# Build a release archive of the `midwrite` executable for the host
# platform: target/package/midwrite-<version>-<target>.tar.gz plus a
# .sha256 file. Builds with --locked; publishes nothing.
#
#   scripts/package-release.sh [target-triple]
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

cargo build --release --locked --target "$target" -p midwrite-cli --bin midwrite

name="midwrite-${version}-${target}"
stage="target/package/${name}"
rm -rf "$stage"
mkdir -p "$stage/runtime"
cp "target/${target}/release/midwrite" "$stage/"
cp README.md LICENSE-MIT LICENSE-APACHE "$stage/"
# The replay client (also vendored by `midwrite init-replay`).
cp -R runtime/node "$stage/runtime/node"
rm -rf "$stage/runtime/node/test"
mkdir -p "$stage/schemas"
cp docs/reference/schemas/trace-report.v1.schema.json docs/reference/schemas/verification-report.v1.schema.json \
   docs/reference/schemas/verification-report.v2.schema.json docs/reference/schemas/regression-report.v1.schema.json \
   docs/reference/schemas/trace-report.v2.schema.json \
   "$stage/schemas/"

# Reproducible archive metadata: fixed owner, sorted entries, mtime of the
# source revision when available.
mtime="$(git log -1 --format=%ct 2>/dev/null || echo 0)"
if tar --version 2>/dev/null | grep -q GNU; then
  tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@${mtime}" -C target/package -czf "target/package/${name}.tar.gz" "$name"
else
  tar --uid 0 --gid 0 -C target/package -czf "target/package/${name}.tar.gz" "$name"
fi
(cd target/package && shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256")
"$stage/midwrite" --version
cat "target/package/${name}.tar.gz.sha256"
