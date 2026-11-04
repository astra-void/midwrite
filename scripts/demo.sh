#!/usr/bin/env bash
# Midwrite Phase 3 demo: static finding -> failure trace -> controlled
# injection -> observed state -> verification report -> HTML view.
#
#   scripts/demo.sh [OUTPUT_DIR]      (default: demo-output/)
#
# Needs: a Rust toolchain; for the runtime part also Bun (>= 1.2), the
# PostgreSQL server binaries (initdb, pg_ctl, postgres, psql; or set
# MIDWRITE_PG_BIN) and macOS (sandbox-exec is used for network isolation).
# Only first-party harnesses under fixtures/replay/ are executed.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$here/demo-output}"
mkdir -p "$out"
cd "$here"

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

step "1. Build"
cargo build --release --quiet
bin="$here/target/release/midwrite"
harness=fixtures/replay/order-vulnerable

step "2. Static analysis of the vulnerable application (nothing is executed)"
"$bin" check "$harness/src" --config "$harness/midwrite.json" --fail-on never

step "3. The failure trace behind each finding (static candidates)"
"$bin" trace "$harness/src" --config "$harness/midwrite.json"
"$bin" trace "$harness/src" --config "$harness/midwrite.json" --format json > "$out/traces.json"
"$bin" report "$out/traces.json" -o "$out/traces.html"
echo "wrote $out/traces.json and $out/traces.html"

missing=""
command -v bun >/dev/null || missing="bun"
if [[ -z "${MIDWRITE_PG_BIN:-}" ]] && ! command -v postgres >/dev/null && ! command -v pg_config >/dev/null; then
  missing="$missing PostgreSQL"
fi
[[ "$(uname)" == "Darwin" ]] || missing="$missing macOS-sandbox-exec"
if [[ -n "$missing" ]]; then
  step "Runtime part SKIPPED: missing$missing"
  exit 0
fi

step "4. Replay without authorization: only the plan is printed (exit status 2)"
"$bin" replay "$harness" || true

step "5. Kill the process right after the PostgreSQL commit, before publishing"
"$bin" replay "$harness" --allow-execution --case kill-after-db-commit

step "6. Every case of the vulnerable harness, 3 times each from a fresh database"
"$bin" replay "$harness" --allow-execution --repeat 3 --output "$out/order-vulnerable.json" > "$out/order-vulnerable.txt"
tail -n 2 "$out/order-vulnerable.txt"

step "7. The same failure points against the transactional-outbox version"
"$bin" replay fixtures/replay/order-outbox --allow-execution --repeat 3 --output "$out/order-outbox.json" > "$out/order-outbox.txt"
tail -n 2 "$out/order-outbox.txt"

step "8. HTML views (self-contained, no scripts, open them in any browser)"
"$bin" report "$out/order-vulnerable.json" -o "$out/order-vulnerable.html"
"$bin" report "$out/order-outbox.json" -o "$out/order-outbox.html"
ls -1 "$out"
