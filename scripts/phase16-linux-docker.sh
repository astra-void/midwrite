#!/usr/bin/env bash
# Phase 16 (derived from scripts/phase15-linux-docker.sh, adds the Phase 16
# retry application, the pinned zod runtimes of the zod differential, the
# schema-required tests and the Phase 16 evidence): run the quality gates, the full test suite with required replay
# and source-verification tests, and the Phase 12 and Phase 13 original-code evidence on
# Linux inside a container (ci/linux-replay.Dockerfile: Debian 12,
# PostgreSQL 15, Node 22, bubblewrap). The repository is mounted read-only
# and copied into the container; the source digest of the copy is recorded
# (scripts/phase8-digest.py) so that it can be compared with the macOS run.
# Only the output directory ($1, default reports/phase12/linux) is written.
# The image build and `npm ci` download packages (Debian, rustup, crates,
# npm registry); the tests themselves run without network.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$repo/reports/phase16/linux}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
image="commitrift-linux-replay:phase4"

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
  "$COMMITRIFT_PG_BIN/postgres" --version
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
  (cd fixtures/phase13/request-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase14/request-app && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  (cd fixtures/phase15/worlds-app && rm -f node_modules && ln -s ../../phase12/workspace-app/node_modules node_modules && npm run build)
  (cd fixtures/phase16/retry && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  # The zod differential runs the pinned runtimes (zod 3.25.76 and 4.6.5).
  (mkdir -p /tmp/zod3 /tmp/zod4 && cd /tmp/zod3 && npm install --no-audit --no-fund zod@3.25.76 && cd /tmp/zod4 && npm install --no-audit --no-fund zod@4.6.5)
} > /out/npm-ci.txt 2>&1; echo "exit $?" >> /out/npm-ci.txt
export COMMITRIFT_ZOD3_DIR=/tmp/zod3/node_modules/zod COMMITRIFT_ZOD4_DIR=/tmp/zod4/node_modules/zod
python3 scripts/phase8-digest.py > /out/digest.txt 2>&1
cargo fmt --all -- --check > /out/fmt.txt 2>&1; echo "exit $?" >> /out/fmt.txt
cargo clippy --workspace --all-targets -- -D warnings > /out/clippy.txt 2>&1; echo "exit $?" >> /out/clippy.txt
COMMITRIFT_REQUIRE_REPLAY=1 COMMITRIFT_REQUIRE_SCHEMA=1 COMMITRIFT_REQUIRE_ZOD=1 cargo test --workspace --no-fail-fast > /out/cargo-test.txt 2>&1; echo "exit $?" >> /out/cargo-test.txt
cargo build --release > /out/build-release.txt 2>&1; echo "exit $?" >> /out/build-release.txt
sha256sum target/release/commitrift >> /out/build-release.txt
COMMITRIFT_BIN=$PWD/target/release/commitrift python3 scripts/phase12-evidence.py --repeat 3 > /out/phase12-evidence.txt 2>&1; echo "exit $?" >> /out/phase12-evidence.txt
cp -r reports/phase12/evidence /out/phase12-evidence
python3 scripts/validate-schemas.py --builtin reports/phase12/evidence/runtime-*.json > /out/phase12-schemas.txt 2>&1; echo "exit $?" >> /out/phase12-schemas.txt
COMMITRIFT_BIN=$PWD/target/release/commitrift python3 scripts/phase13-evidence.py --repeat 3 > /out/phase13-evidence.txt 2>&1; echo "exit $?" >> /out/phase13-evidence.txt
cp -r reports/phase13/evidence /out/phase13-evidence
python3 scripts/validate-schemas.py --builtin reports/phase13/evidence/*.json > /out/phase13-schemas.txt 2>&1; echo "exit $?" >> /out/phase13-schemas.txt
python3 scripts/phase13-lock-matrix.py --pg-bin "$COMMITRIFT_PG_BIN" > /out/phase13-lock-matrix.txt 2>&1; echo "exit $?" >> /out/phase13-lock-matrix.txt
COMMITRIFT_BIN=$PWD/target/release/commitrift python3 scripts/phase14-evidence.py --repeat 3 > /out/phase14-evidence.txt 2>&1; echo "exit $?" >> /out/phase14-evidence.txt
cp -r reports/phase14/evidence /out/phase14-evidence
COMMITRIFT_BIN=$PWD/target/release/commitrift python3 scripts/phase15-evidence.py /out/phase15-evidence --repeat 3 > /out/phase15-evidence.txt 2>&1; echo "exit $?" >> /out/phase15-evidence.txt
COMMITRIFT_BIN=$PWD/target/release/commitrift python3 scripts/phase16-evidence.py /out/phase16-evidence --repeat 3 > /out/phase16-evidence.txt 2>&1; echo "exit $?" >> /out/phase16-evidence.txt
target/release/commitrift discover fixtures/phase16/retry --model-check --format json --output /out/retry-discovery.json > /dev/null 2>&1
python3 scripts/validate-schemas.py --builtin --schema docs/phase16/schemas/discovery-report.v1.schema.json /out/retry-discovery.json > /out/phase16-schemas.txt 2>&1; echo "exit $?" >> /out/phase16-schemas.txt
cp reports/phase13/lock-matrix/summary.md /out/lock-matrix-summary.md 2>/dev/null || true
'
echo "logs in $out"
