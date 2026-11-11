#!/usr/bin/env bash
# Run the quality gates, the full test suite with required replay,
# source-verification, schema and zod tests, the PostgreSQL lock matrix and
# original-code evidence on Linux inside a container
# (ci/linux-replay.Dockerfile: Debian 12, PostgreSQL 15, Node 22, bubblewrap).
#
#   scripts/linux-docker.sh [OUT_DIR] [SUITE...]
#
# OUT_DIR defaults to reports/linux. SUITE names suites of scripts/evidence.py
# (default: workspace responses retry-wrapper worlds; `all` runs every suite,
# including crash-retry, in-flight and connection-loss). Each suite writes
# OUT_DIR/evidence/<suite>/ and OUT_DIR/evidence-<suite>.txt; model-check
# reports are then checked with scripts/validate-schemas.py or
# scripts/report-check.py.
#
# The repository is mounted read-only and copied into the container; the
# source digest of the copy is recorded (scripts/digest.py) so that it can be
# compared with the macOS run. Only the output directory is written. The
# image build and `npm ci` download packages (Debian, rustup, crates, npm
# registry); the tests themselves run without network.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$repo/reports/linux}"
shift || true
suites="${*:-workspace responses retry-wrapper worlds}"
if [[ "$suites" == "all" ]]; then
  suites="crash-retry in-flight connection-loss commerce workspace responses retry-wrapper worlds"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
image="midwrite-linux-replay"

docker build -q -t "$image" -f "$repo/ci/linux-replay.Dockerfile" "$repo/ci" >/dev/null

docker run --rm --security-opt seccomp=unconfined --security-opt systempaths=unconfined \
  -v "$repo:/src:ro" -v "$out:/out" -e SUITES="$suites" "$image" bash -c '
set -uo pipefail
mkdir -p work
# .git is kept: the isolation backend finds the project root (and with it
# the `file:` dependency on runtime/node) by the enclosing Git work tree.
tar -C /src --exclude=./target --exclude=./.corpora --exclude="*/node_modules" --exclude="*/dist" -cf - . | tar -C work -xf -
cd work
{
  uname -a
  head -2 /etc/os-release
  rustc --version; node --version; bwrap --version
  "$MIDWRITE_PG_BIN/postgres" --version
} > /out/environment.txt 2>&1
{
  (cd fixtures/replay/drizzle-node && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  for app in team-app shop-app pgjs-app ledger-app ledger-live ledger-pgjs commerce-app workspace-app \
             defaults-probe request-app retry-wrapper-app retry; do
    (cd "fixtures/apps/$app" && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
  done
  # The worlds application links the workspace application'"'"'s packages.
  (cd fixtures/apps/worlds-app && rm -f node_modules && ln -s ../workspace-app/node_modules node_modules && npm run build)
  # The zod differential runs the pinned runtimes (zod 3.25.76 and 4.6.5).
  (mkdir -p /tmp/zod3 /tmp/zod4 && cd /tmp/zod3 && npm install --no-audit --no-fund zod@3.25.76 && cd /tmp/zod4 && npm install --no-audit --no-fund zod@4.6.5)
} > /out/npm-ci.txt 2>&1; echo "exit $?" >> /out/npm-ci.txt
export MIDWRITE_ZOD3_DIR=/tmp/zod3/node_modules/zod MIDWRITE_ZOD4_DIR=/tmp/zod4/node_modules/zod
python3 scripts/digest.py > /out/digest.txt 2>&1
cargo fmt --all -- --check > /out/fmt.txt 2>&1; echo "exit $?" >> /out/fmt.txt
cargo clippy --workspace --all-targets -- -D warnings > /out/clippy.txt 2>&1; echo "exit $?" >> /out/clippy.txt
MIDWRITE_REQUIRE_REPLAY=1 MIDWRITE_REQUIRE_SCHEMA=1 MIDWRITE_REQUIRE_ZOD=1 cargo test --workspace --no-fail-fast > /out/cargo-test.txt 2>&1; echo "exit $?" >> /out/cargo-test.txt
cargo build --release > /out/build-release.txt 2>&1; echo "exit $?" >> /out/build-release.txt
sha256sum target/release/midwrite >> /out/build-release.txt
export MIDWRITE_BIN=$PWD/target/release/midwrite
for suite in $SUITES; do
  dir=/out/evidence/$suite
  python3 scripts/evidence.py "$suite" --out "$dir" --repeat 3 > "/out/evidence-$suite.txt" 2>&1; echo "exit $?" >> "/out/evidence-$suite.txt"
  check=/out/evidence-$suite.check.txt
  case "$suite" in
    in-flight) python3 scripts/report-check.py "$dir"/*.json > "$check" 2>&1; echo "exit $?" >> "$check" ;;
    crash-retry|connection-loss|responses) python3 scripts/validate-schemas.py --builtin "$dir"/*.json > "$check" 2>&1; echo "exit $?" >> "$check" ;;
    commerce|workspace) python3 scripts/validate-schemas.py --builtin "$dir"/runtime-*.json > "$check" 2>&1; echo "exit $?" >> "$check" ;;
  esac
done
python3 scripts/lock-matrix.py --pg-bin "$MIDWRITE_PG_BIN" --results /out/lock-matrix/results.json --summary /out/lock-matrix/summary.md > /out/lock-matrix.txt 2>&1; echo "exit $?" >> /out/lock-matrix.txt
target/release/midwrite discover fixtures/apps/retry --model-check --format json --output /out/retry-discovery.json > /dev/null 2>&1
python3 scripts/validate-schemas.py --builtin --schema docs/reference/schemas/discovery-report.v1.schema.json /out/retry-discovery.json > /out/discovery-schema.txt 2>&1; echo "exit $?" >> /out/discovery-schema.txt
'
echo "logs in $out"
