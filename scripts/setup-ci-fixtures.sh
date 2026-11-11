#!/usr/bin/env bash
# Install and compile the locked first-party apps used by workspace tests.
# External corpora and frozen evaluation applications are never executed.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

for app in \
  fixtures/replay/drizzle-node \
  fixtures/apps/team-app \
  fixtures/apps/shop-app \
  fixtures/apps/pgjs-app \
  fixtures/apps/ledger-app \
  fixtures/apps/ledger-live \
  fixtures/apps/ledger-pgjs \
  fixtures/apps/commerce-app \
  fixtures/apps/workspace-app \
  fixtures/apps/defaults-probe \
  fixtures/apps/request-app \
  fixtures/apps/retry-wrapper-app \
  fixtures/apps/retry
do
  (cd "$app" && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
done

# This app deliberately reuses the workspace fixture's pinned dependencies.
if [[ ! -e fixtures/apps/worlds-app/node_modules && ! -L fixtures/apps/worlds-app/node_modules ]]; then
  ln -s ../workspace-app/node_modules fixtures/apps/worlds-app/node_modules
fi
(cd fixtures/apps/worlds-app && npm run build)
