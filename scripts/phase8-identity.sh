#!/usr/bin/env bash
# Phase 8 regression: `commitrift check` (json, sarif, text) and
# `commitrift trace` (json) of the Phase 8 binary must be byte-identical to
# the Phase 7 binary on every corpus project, at depths 3, 6 and 12,
# in every path mode (shared, compact, legacy) of the new binary.
# Usage: scripts/phase8-identity.sh PHASE7_BINARY [OUT_FILE]
set -euo pipefail
old="$1"
out="${2:-/dev/stdout}"
root="$(cd "$(dirname "$0")/.." && pwd)"
new="$root/target/release/commitrift"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
projects=(
  "saas-starter|$root/.corpora/saas-starter|$root/corpora/configs/saas-starter.json"
  "midday|$root/.corpora/midday|$root/corpora/configs/midday.json"
  "school-ledger|/Users/returnf4lse/Desktop/Workspace/nextjs/school-ledger|$root/corpora/configs/school-ledger.json"
  "unnamed-ai-project|/Users/returnf4lse/Desktop/Workspace/nextjs/unnamed-ai-project|$root/corpora/configs/unnamed-ai-project.json"
)
status=0
{
for entry in "${projects[@]}"; do
  IFS='|' read -r name path config <<<"$entry"
  [ -d "$path" ] || { echo "$name: not available (skipped)"; continue; }
  for depth in 3 6 12; do
    for mode in ${MODES:-compact legacy}; do
      for fmt in json sarif text; do
        (cd "$path" && "$old" check . --config "$config" --format "$fmt" --max-call-depth "$depth" >"$tmp/old.$fmt" 2>/dev/null) || true
        (cd "$path" && COMMITRIFT_PATHS=$mode "$new" check . --config "$config" --format "$fmt" --max-call-depth "$depth" >"$tmp/new.$fmt" 2>/dev/null) || true
        if cmp -s "$tmp/old.$fmt" "$tmp/new.$fmt"; then r=identical; else r=DIFFERENT; status=1; fi
        echo "$name depth=$depth mode=$mode check --format $fmt: $r ($(wc -c <"$tmp/new.$fmt" | tr -d ' ') bytes, sha256 $(shasum -a 256 "$tmp/new.$fmt" | cut -c1-16))"
      done
      (cd "$path" && "$old" trace . --config "$config" --format json --max-call-depth "$depth" >"$tmp/old.trace" 2>/dev/null) || true
      (cd "$path" && COMMITRIFT_PATHS=$mode "$new" trace . --config "$config" --format json --max-call-depth "$depth" >"$tmp/new.trace" 2>/dev/null) || true
      if cmp -s "$tmp/old.trace" "$tmp/new.trace"; then r=identical; else r=DIFFERENT; status=1; fi
      echo "$name depth=$depth mode=$mode trace --format json: $r ($(wc -c <"$tmp/new.trace" | tr -d ' ') bytes)"
    done
  done
done
} >"$out"
exit $status
