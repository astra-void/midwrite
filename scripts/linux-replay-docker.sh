#!/usr/bin/env bash
# Run CommitRift's quality gates and required replay tests on Linux inside a
# container (see ci/linux-replay.Dockerfile). The repository is mounted
# read-only and copied into the container; nothing is written back except
# the log directory given as $1 (default: reports/phase4/linux).
#
#   scripts/linux-replay-docker.sh [output-dir] [--default-seccomp]
#
# --default-seccomp runs with Docker's default seccomp profile, under which
# unprivileged user namespaces are forbidden: replay must then be BLOCKED and
# the required replay tests must FAIL (the negative check).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$repo/reports/phase4/linux}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
image="commitrift-linux-replay:phase4"

security=(--security-opt seccomp=unconfined --security-opt systempaths=unconfined)
mode="isolated"
if [[ "${2:-}" == "--default-seccomp" ]]; then
  security=()
  mode="default-seccomp"
fi

docker build -q -t "$image" -f "$repo/ci/linux-replay.Dockerfile" "$repo/ci" >/dev/null

docker run --rm ${security[@]+"${security[@]}"} \
  -v "$repo:/src:ro" -v "$out:/out" \
  -e MODE="$mode" \
  "$image" bash -c '
set -uo pipefail
mkdir -p work
# Copy the source without host build outputs, dependencies or corpora.
tar -C /src --exclude=./target --exclude=./.corpora --exclude="*/node_modules" --exclude="*/dist" -cf - . | tar -C work -xf -
cd work
{
  uname -a
  cat /etc/os-release | head -2
  rustc --version; node --version; bun --version; bwrap --version
  "$COMMITRIFT_PG_BIN/postgres" --version
  echo "max_user_namespaces=$(cat /proc/sys/user/max_user_namespaces)"
} > /out/environment-$MODE.txt 2>&1
(cd fixtures/replay/drizzle-node && npm ci --ignore-scripts --no-audit --no-fund && npm run build) > /out/npm-ci-$MODE.txt 2>&1
if [[ "$MODE" == "isolated" ]]; then
  cargo fmt --all -- --check > /out/fmt.txt 2>&1; echo "exit $?" >> /out/fmt.txt
  cargo clippy --workspace --all-targets -- -D warnings > /out/clippy.txt 2>&1; echo "exit $?" >> /out/clippy.txt
fi
COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace > /out/cargo-test-$MODE.txt 2>&1; echo "exit $?" >> /out/cargo-test-$MODE.txt
cargo build --release -q 2>/dev/null
./target/release/commitrift replay fixtures/replay/drizzle-node --allow-execution --format json --output /out/drizzle-node-$MODE.json > /dev/null 2> /out/drizzle-node-$MODE.stderr.txt
echo "exit $?" >> /out/drizzle-node-$MODE.stderr.txt
if [[ "$MODE" == "isolated" ]]; then
  ./target/release/commitrift replay fixtures/replay/drizzle-node --allow-execution --case a-kill-after-db-commit --repeat 5 --format json --output /out/drizzle-node-repeat5.json > /dev/null 2>&1
  echo "exit $?" > /out/drizzle-node-repeat5.exit.txt
  (cd fixtures/replay/drizzle-node/generated-harness && node ../node_modules/typescript/bin/tsc -p tsconfig.json) && ./target/release/commitrift replay fixtures/replay/drizzle-node/generated-harness --allow-execution --format json --output /out/generated-harness.json > /dev/null 2>&1
  echo "exit $?" > /out/generated-harness.exit.txt
  scripts/package-release.sh > /out/package-release.txt 2>&1; echo "exit $?" >> /out/package-release.txt
  ./target/package/commitrift-*/commitrift check fixtures --config fixtures/commitrift.json --format json --fail-on never | python3 -c "import json,sys; r=json.load(sys.stdin); print(\"packaged binary: check fixtures:\", r[\"summary\"])" >> /out/package-release.txt 2>&1
fi
'
echo "logs in $out"
