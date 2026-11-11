#!/usr/bin/env bash
# Provision isolated replay on disposable Linux GitHub-hosted runners.
set -euo pipefail
: "${GITHUB_ENV:?GitHub Actions environment file is required}"

sudo apt-get update
sudo apt-get install -y --no-install-recommends postgresql bubblewrap
pg_bin="$(ls -d /usr/lib/postgresql/*/bin | sort -V | tail -1)"
echo "MIDWRITE_PG_BIN=$pg_bin" >> "$GITHUB_ENV"

# Ubuntu's AppArmor policy otherwise blocks bubblewrap's user namespaces.
if [[ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]]; then
  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
fi
bwrap --unshare-user --unshare-net --unshare-pid --ro-bind / / --dev /dev --proc /proc true
