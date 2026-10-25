#!/usr/bin/env bash
# Clone (or update) the external corpus projects listed in corpora/manifest.json
# at their pinned revisions into .corpora/. Nothing is built or executed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
manifest="$here/manifest.json"
count="$(jq '.projects | length' "$manifest")"

for ((i = 0; i < count; i++)); do
  origin="$(jq -r ".projects[$i].origin" "$manifest")"
  [[ "$origin" == "external" ]] || continue
  name="$(jq -r ".projects[$i].name" "$manifest")"
  repo="$(jq -r ".projects[$i].repository" "$manifest")"
  rev="$(jq -r ".projects[$i].revision" "$manifest")"
  dest="$here/$(jq -r ".projects[$i].path" "$manifest")"
  if [[ ! -d "$dest/.git" ]]; then
    git init -q "$dest"
    git -C "$dest" remote add origin "$repo"
  fi
  echo "fetching $name @ $rev"
  git -C "$dest" fetch -q --depth 1 origin "$rev"
  git -C "$dest" checkout -q --detach FETCH_HEAD
  actual="$(git -C "$dest" rev-parse HEAD)"
  if [[ "$actual" != "$rev" ]]; then
    echo "error: $name is at $actual, expected $rev" >&2
    exit 1
  fi
done
