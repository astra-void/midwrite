#!/usr/bin/env bash
# Static-analysis regression: `midwrite check` (json, sarif, text) and
# `midwrite trace` (json) of this build must be byte-identical to an earlier
# binary on every corpus project, at each call depth (DEPTHS, default
# "3 6 12") and in each path mode of the new binary (MODES, default
# "compact legacy"; `shared` is the third mode).
#
#   scripts/check-identity.sh OLD_BINARY [OUT_FILE]
#
# Public projects come from .corpora/ (corpora/fetch.sh); the private ones
# are looked up under MIDWRITE_PRIVATE_ROOT (default
# ~/Desktop/Workspace/nextjs) and skipped when absent. Exit status 1 on any
# difference.
set -euo pipefail
old="${1:?usage: scripts/check-identity.sh OLD_BINARY [OUT_FILE]}"
out="${2:-/dev/stdout}"
root="$(cd "$(dirname "$0")/.." && pwd)"
new="$root/target/release/midwrite"
private="${MIDWRITE_PRIVATE_ROOT:-$HOME/Desktop/Workspace/nextjs}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
projects=(
  "saas-starter|$root/.corpora/saas-starter|$root/corpora/configs/saas-starter.json"
  "midday|$root/.corpora/midday|$root/corpora/configs/midday.json"
  "school-ledger|$private/school-ledger|$root/corpora/configs/school-ledger.json"
  "unnamed-ai-project|$private/unnamed-ai-project|$root/corpora/configs/unnamed-ai-project.json"
)
status=0
{
for entry in "${projects[@]}"; do
  IFS='|' read -r name path config <<<"$entry"
  [ -d "$path" ] || { echo "$name: not available (skipped)"; continue; }
  for depth in ${DEPTHS:-3 6 12}; do
    for mode in ${MODES:-compact legacy}; do
      for fmt in json sarif text; do
        (cd "$path" && "$old" check . --config "$config" --format "$fmt" --max-call-depth "$depth" >"$tmp/old.$fmt" 2>/dev/null) || true
        (cd "$path" && MIDWRITE_PATHS=$mode "$new" check . --config "$config" --format "$fmt" --max-call-depth "$depth" >"$tmp/new.$fmt" 2>/dev/null) || true
        if cmp -s "$tmp/old.$fmt" "$tmp/new.$fmt"; then r=identical; else r=DIFFERENT; status=1; fi
        echo "$name depth=$depth mode=$mode check --format $fmt: $r ($(wc -c <"$tmp/new.$fmt" | tr -d ' ') bytes, sha256 $(shasum -a 256 "$tmp/new.$fmt" | cut -c1-16))"
      done
      (cd "$path" && "$old" trace . --config "$config" --format json --max-call-depth "$depth" >"$tmp/old.trace" 2>/dev/null) || true
      (cd "$path" && MIDWRITE_PATHS=$mode "$new" trace . --config "$config" --format json --max-call-depth "$depth" >"$tmp/new.trace" 2>/dev/null) || true
      if cmp -s "$tmp/old.trace" "$tmp/new.trace"; then r=identical; else r=DIFFERENT; status=1; fi
      echo "$name depth=$depth mode=$mode trace --format json: $r ($(wc -c <"$tmp/new.trace" | tr -d ' ') bytes)"
    done
  done
done
} >"$out"
exit $status
