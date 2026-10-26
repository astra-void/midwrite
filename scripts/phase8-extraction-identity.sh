#!/usr/bin/env bash
# Phase 8 regression for the shared semantic-facts layer: every Phase 6/7
# model-checking scenario (extracted from TypeScript or declared) must give
# the same model-check report (JSON, without timing) with the Phase 8
# binary as with the Phase 7 binary, apart from the tool version.
# Usage: scripts/phase8-extraction-identity.sh PHASE7_BINARY [OUT_FILE]
set -euo pipefail
old="$1"
out="${2:-/dev/stdout}"
root="$(cd "$(dirname "$0")/.." && pwd)"
new="$root/target/release/commitrift"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
status=0
normalize() {
  python3 -c 'import json,sys; d=json.load(sys.stdin); d.get("tool",{}).pop("version",None); print(json.dumps(d,sort_keys=True))'
}
{
for scenario in "$root"/fixtures/concurrency/scenarios/*.json "$root"/fixtures/phase7/scenarios/*.json; do
  name="${scenario#"$root"/}"
  "$old" model-check "$scenario" --format json --no-timing 2>/dev/null | normalize >"$tmp/old.json" || true
  "$new" model-check "$scenario" --format json --no-timing 2>/dev/null | normalize >"$tmp/new.json" || true
  if [ -s "$tmp/old.json" ] && cmp -s "$tmp/old.json" "$tmp/new.json"; then r=identical; else r=DIFFERENT; status=1; fi
  echo "$name: $r"
done
} >"$out"
exit $status
