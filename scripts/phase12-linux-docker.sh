#!/usr/bin/env bash
# Phase 12 (derived from scripts/phase10-linux-docker.sh): run the quality gates, the full test suite with required replay
# and source-verification tests, and the Phase 12 original-code evidence on
# Linux inside a container (ci/linux-replay.Dockerfile: Debian 12,
# PostgreSQL 15, Node 22, bubblewrap). The repository is mounted read-only
# and copied into the container; the source digest of the copy is recorded
# (scripts/phase8-digest.py) so that it can be compared with the macOS run.
# Only the output directory ($1, default reports/phase12/linux) is written.
# The image build and `npm ci` download packages (Debian, rustup, crates,
# npm registry); the tests themselves run without network.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$repo/reports/phase12/linux}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
image="midwrite-linux-replay:phase4"

docker build -q -t "$image" -f "$repo/ci/linux-replay.Dockerfile" "$repo/ci" >/dev/null

docker run --rm --security-opt seccomp=unconfined --security-opt systempaths=unconfined \
  -v "$repo:/src:ro" -v "$out:/out" "$image" bash -c '
set -uo pipefail
mkdir -p work
# .git is kept: the isolation backend finds the project root (and with it
# the `file:` dependency on runtime/node) by the enclosing Git work tree.
tar -C /src --exclude=./target --exclude=./.corpora --exclude="*/node_modules" --exclude="*/dist" -cf - . | tar -C work -xf -
cd work
python3 scripts/phase8-digest.py > /out/digest.txt
{
  uname -a
  head -2 /etc/os-release
  rustc --version; node --version; bwrap --version
  "$MIDWRITE_PG_BIN/postgres" --version
} > /out/environment.txt 2>&1
{
  (cd fixtures/replay/drizzle-node && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase7/team-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase8/shop-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase8/pgjs-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase9/ledger-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase10/ledger-live && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase10/ledger-pgjs && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase11/commerce-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase12/workspace-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase12/defaults-probe && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
} > /out/npm-ci.txt 2>&1; echo "exit $?" >> /out/npm-ci.txt
python3 scripts/phase8-digest.py > /out/digest.txt 2>&1
cargo fmt --all -- --check > /out/fmt.txt 2>&1; echo "exit $?" >> /out/fmt.txt
cargo clippy --workspace --all-targets -- -D warnings > /out/clippy.txt 2>&1; echo "exit $?" >> /out/clippy.txt
MIDWRITE_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast > /out/cargo-test.txt 2>&1; echo "exit $?" >> /out/cargo-test.txt
cargo build --release > /out/build-release.txt 2>&1; echo "exit $?" >> /out/build-release.txt
sha256sum target/release/midwrite >> /out/build-release.txt
MIDWRITE_BIN=$PWD/target/release/midwrite python3 scripts/phase12-evidence.py --repeat 3 > /out/phase12-evidence.txt 2>&1; echo "exit $?" >> /out/phase12-evidence.txt
cp -r reports/phase12/evidence /out/phase12-evidence
python3 scripts/validate-schemas.py --builtin reports/phase12/evidence/runtime-*.json > /out/phase12-schemas.txt 2>&1; echo "exit $?" >> /out/phase12-schemas.txt
'
echo "logs in $out"
