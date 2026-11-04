#!/usr/bin/env bash
# Phase 16: run every command of the Closed Alpha walkthrough
# (docs/alpha/README.md) from a clean output directory and check what the
# guide says each one shows. Exit 0 only if every step behaves as documented.
#
# Usage: scripts/phase16-alpha-check.sh OUT_DIR
#
# Needs the release binary (step 1 builds it), Node 22 and the PostgreSQL
# server binaries (PATH or MIDWRITE_PG_BIN) for the optional step 8.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$(mkdir -p "$1" && cd "$1" && pwd)"
app="$root/fixtures/phase16/retry"
bin="$root/target/release/midwrite"
log="$out/alpha-check.log"
: >"$log"
fail=0
step() { printf '\n== %s\n' "$1" | tee -a "$log"; }
expect() { # expect FILE PATTERN DESCRIPTION
  if grep -q -- "$2" "$1"; then echo "ok: $3" | tee -a "$log"; else echo "FAIL: $3 (missing: $2)" | tee -a "$log"; fail=1; fi
}

step "1. build"
(cd "$root" && cargo build --release -p midwrite-cli) >>"$log" 2>&1 || { echo "FAIL: build" | tee -a "$log"; exit 1; }
"$bin" --version | tee -a "$log"

step "2. discover with model checking"
"$bin" discover "$app" --model-check >"$out/discover.txt" 2>&1
expect "$out/discover.txt" "application code executed: no" "discovery never executes code"
expect "$out/discover.txt" "CR-SYNTH-65F47A  \[MODEL_COUNTEREXAMPLE\]" "placeOrder retry candidate is a model counterexample"
expect "$out/discover.txt" "src/services/orders.ts:16:13" "the conflict names the source location"

step "3. property origin: declared vs hypothesized"
expect "$out/discover.txt" "origin: USER_DECLARED" "a declared property is labeled USER_DECLARED"
expect "$out/discover.txt" "origin: SCHEMA_ENFORCED" "a schema-enforced property is labeled SCHEMA_ENFORCED"

step "4. input and schedule coverage"
"$bin" discover "$app" --model-check --explain-coverage >"$out/coverage.txt" 2>&1
expect "$out/coverage.txt" "Input domain exhaustive: no" "coverage says inputs were not exhausted"
expect "$out/coverage.txt" "Schedule exploration" "coverage reports schedule exploration"
expect "$out/coverage.txt" "loop BOUNDED_UNROLLING" "the retry loop's bound is reported"

step "5. emit and replay the counterexample in the model"
rm -rf "$out/scenarios"
"$bin" discover "$app" --model-check --emit-scenarios "$out/scenarios" >/dev/null 2>&1
scenario="$out/scenarios/cr-synth-65f47a-actual.json"
[ -f "$scenario" ] && echo "ok: scenario written" | tee -a "$log" || { echo "FAIL: $scenario missing" | tee -a "$log"; fail=1; }
[ -f "${scenario%.json}.counterexample.json" ] && echo "ok: reproduction artifact written" | tee -a "$log" || { echo "FAIL: reproduction artifact missing" | tee -a "$log"; fail=1; }
"$bin" model-check "$scenario" >"$out/model-check.txt" 2>&1
expect "$out/model-check.txt" "one-order-per-request" "model-check replays the violation of the declared property"

step "6. unsupported behavior is explained"
"$bin" discover "$root/fixtures/phase16/loops" >"$out/unsupported.txt" 2>&1
expect "$out/unsupported.txt" "loop" "loop shapes outside the subset are named"

step "7. JSON report validates against the published schema"
"$bin" discover "$app" --model-check --format json --output "$out/discover.json" >/dev/null 2>&1
python3 "$root/scripts/validate-schemas.py" --builtin --schema "$root/docs/phase16/schemas/discovery-report.v1.schema.json" "$out/discover.json" >"$out/schema.txt" 2>&1
expect "$out/schema.txt" "0 schema errors" "discovery JSON validates"

step "8. (optional, authorized) original-code verification"
if command -v node >/dev/null && { [ -n "${MIDWRITE_PG_BIN:-}" ] || command -v postgres >/dev/null || [ -x /opt/homebrew/opt/postgresql@17/bin/postgres ]; }; then
  export PATH="${MIDWRITE_PG_BIN:-/opt/homebrew/opt/postgresql@17/bin}:$PATH"
  (cd "$app" && npm ci --offline >/dev/null 2>&1 && npm run build >/dev/null 2>&1)
  "$bin" model-check "$scenario" --verify-source --allow-execution --source-repeat 2 >"$out/verify.txt" 2>&1
  expect "$out/verify.txt" "reproduced" "the counterexample reproduces on the original code"
else
  echo "skipped: Node or PostgreSQL server binaries not available" | tee -a "$log"
fi

echo
[ "$fail" = 0 ] && echo "alpha walkthrough: every documented step behaved as described" | tee -a "$log" || echo "alpha walkthrough: FAILURES (see $log)" | tee -a "$log"
exit "$fail"
