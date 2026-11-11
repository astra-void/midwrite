#!/usr/bin/env bash
# Install and compile the locked first-party apps used by workspace tests.
# External corpora and frozen evaluation applications are never executed.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

for app in \
  fixtures/replay/drizzle-node \
  fixtures/phase7/team-app \
  fixtures/phase8/shop-app \
  fixtures/phase8/pgjs-app \
  fixtures/phase9/ledger-app \
  fixtures/phase10/ledger-live \
  fixtures/phase10/ledger-pgjs \
  fixtures/phase11/commerce-app \
  fixtures/phase12/workspace-app \
  fixtures/phase12/defaults-probe \
  fixtures/phase13/request-app \
  fixtures/phase14/request-app \
  fixtures/phase16/retry
do
  (cd "$app" && npm ci --ignore-scripts --no-audit --no-fund && npm run build)
done

# This app deliberately reuses the workspace fixture's pinned dependencies.
if [[ ! -e fixtures/phase15/worlds-app/node_modules && ! -L fixtures/phase15/worlds-app/node_modules ]]; then
  ln -s ../../phase12/workspace-app/node_modules fixtures/phase15/worlds-app/node_modules
fi
(cd fixtures/phase15/worlds-app && npm run build)
