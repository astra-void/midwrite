#!/usr/bin/env bash
# Midwrite Phase 4 demo: real TypeScript + Drizzle ORM code ->
# MW1001 -> failure trace -> generated replay harness -> declared invariant
# -> authorized, isolated replay -> surviving PostgreSQL state -> HTML ->
# comparison with a transactional outbox.
#
#   scripts/demo-phase4.sh [OUTPUT_DIR]      (default: demo-output/phase4)
#
# Needs a Rust toolchain; Node.js >= 22 and npm (the fixture's pinned
# dependencies are installed with `npm ci` from its lockfile); PostgreSQL
# server binaries (or MIDWRITE_PG_BIN); OS isolation: macOS sandbox-exec,
# or Linux bubblewrap with unprivileged user namespaces.
# Only first-party harnesses under fixtures/replay/ are executed.
#
# Automated:  every command below.
# Manual:     step 5. Writing the observations, the invariant, the schema
#             and the scenario call cannot be automated; the demo shows the
#             developer's completed version (fixtures/replay/drizzle-node/
#             generated-harness) next to a freshly generated skeleton.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$here/demo-output/phase4}"
mkdir -p "$out"
cd "$here"
app=fixtures/replay/drizzle-node
skeleton="$app/.gen-demo-$$"
trap 'rm -rf "$skeleton"' EXIT

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

step "0. Build Midwrite and install the fixture's pinned dependencies (npm ci, lockfile)"
cargo build --release --quiet
bin="$here/target/release/midwrite"
(cd "$app" && npm ci --ignore-scripts --no-audit --no-fund --silent && npm run build --silent)

step "1. Scan real TypeScript + Drizzle ORM code (nothing is executed)"
(cd "$app" && "$bin" check src --fail-on never)

step "2./3. The failure trace of the MW1001 finding (a static candidate)"
(cd "$app" && "$bin" trace src --format json > "$out/trace.json" && "$bin" trace src --finding 4f53a6215ac16913)
"$bin" report "$out/trace.json" -o "$out/trace.html"

step "4. Generate an editable replay harness from the finding (writes only into the new directory)"
(cd "$app" && "$bin" init-replay --trace "$out/trace.json" --finding 4f53a6215ac16913 --output "$(basename "$skeleton")")
echo "--- replay refuses the unfinished skeleton:"
"$bin" replay "$skeleton" --allow-execution || true

step "5. MANUAL: the developer fills in the marked integration points (completed version shown)"
diff -u "$skeleton/midwrite-replay.json" "$app/generated-harness/midwrite-replay.json" | sed -n '1,60p' || true
(cd "$app/generated-harness" && node ../node_modules/typescript/bin/tsc -p tsconfig.json)

step "6./7. Authorized, isolated replay; observe the surviving PostgreSQL state"
"$bin" replay "$app/generated-harness" --allow-execution --format json --output "$out/generated-harness.json" > /dev/null || true
"$bin" replay "$app/generated-harness" --allow-execution --case kill-after-db-commit --repeat 5 || true

step "8. Render the verification report"
"$bin" report "$out/generated-harness.json" -o "$out/generated-harness.html"
echo "wrote $out/generated-harness.html"

step "9. Compare with the transactional outbox (the same commit point, plus relay failures)"
for c in d-kill-after-tx-commit d-kill-relay-after-publish-before-ack a-kill-after-db-commit; do
  "$bin" replay "$app" --allow-execution --case "$c" | grep -E "^case|after:|afterRecovery:|recovery \(runtime" || true
done
"$bin" replay "$app" --allow-execution --format json --output "$out/drizzle-node.json" > /dev/null || true
"$bin" report "$out/drizzle-node.json" -o "$out/drizzle-node.html"
echo "wrote $out/drizzle-node.html"
