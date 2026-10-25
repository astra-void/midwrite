# Linux replay isolation and CI

Phase 4 milestone 3. Code: `crates/commitrift-replay/src/isolation.rs`.
Evidence: `reports/phase4/linux/`. Threat model: unchanged
([`../phase3/SECURITY.md`](../phase3/SECURITY.md)), reviewed in
[`SECURITY-REVIEW.md`](SECURITY-REVIEW.md).

## Options evaluated (on a real Linux kernel)

Measured in Debian bookworm containers on Docker Desktop (linuxkit kernel
7.0.12, aarch64), as an unprivileged user:

| Mechanism | Default Docker seccomp | seccomp unconfined | seccomp + system paths unconfined |
| --- | --- | --- | --- |
| `unshare --user --net` (util-linux) | refused | works | works |
| bubblewrap 0.8.0 (`--unshare-user --unshare-net --unshare-pid --proc /proc`) | refused | refused: cannot mount a fresh `/proc` over Docker's masked one | **works** |
| Rootless OCI containers (podman) | not installed; would add a second container runtime inside CI for each harness run | | |

Rejected fallback: bind-mounting the host's `/proc` when a fresh procfs
cannot be mounted. Inside a user namespace mapped to the same uid, that
would expose `/proc/<pid>/environ` of the user's other processes — the very
secrets replay keeps out of the harness. Refusing is the fail-closed choice.

**Chosen: bubblewrap.** It is small, maintained, packaged by every major
distribution (`apt-get install bubblewrap`), used by Flatpak, and does
exactly the namespace and mount setup needed, without a daemon. Writing the
same setup by hand with `unshare(2)` and mount syscalls in a `pre_exec` hook
was possible but riskier (allocation after `fork` in a multithreaded
process, ordering of mounts) for no gain.

## The isolation

`isolation::prepare` picks the backend: macOS `sandbox-exec` (Phase 3,
unchanged); Linux bubblewrap; anything else is blocked. The Linux sandbox:

| Aspect | Setting |
| --- | --- |
| Namespaces | new user (same uid), **network (loopback only)**, PID, IPC, UTS (`commitrift-replay`), cgroup (when available) |
| Visible filesystem | read-only: `/usr`, `/bin`, `/sbin`, `/lib*`, `/etc`, `/sys`, `/nix/store` (those that exist; merged-`/usr` symlinks kept as symlinks), the harness project (the enclosing Git work tree, or the harness directory), and the installation directory of the harness runtime (e.g. `/usr/local` or `~/.bun`) |
| Writable | only the private replay root (`/tmp/cr-…`, 0700), bound at the same path, so the PostgreSQL socket in it is reachable |
| Fresh | `/proc` (only the sandbox's processes), minimal `/dev`, private `/tmp` |
| Not mounted | `/run`, `/var` (`/var/run/postgresql`, `/var/run/docker.sock`), `/home` and `/root` except the paths above, the host `/tmp` (`/tmp/.s.PGSQL.5432`) |
| Lifetime | `--die-with-parent`; the PID namespace ends when the harness ends (detached grandchildren die too) |
| Program | resolved on the host (PATH, symlinks) and passed as an absolute path |

**Probe and fail closed.** Before the first run, the exact argument list is
executed with `true`. If bubblewrap is missing or cannot create its
namespaces, replay is **blocked** (exit 3) with the cause and the usual
fixes (`kernel.unprivileged_userns_clone`, `user.max_user_namespaces`,
Ubuntu's `kernel.apparmor_restrict_unprivileged_userns`, container seccomp /
masked `/proc`). Running without isolation still requires both the
manifest's `"network": "allow"` and `--allow-network`, is recorded in the
report (`environment.isolation: "none"`), and is never used by the required
CI tests.

**PostgreSQL connectivity.** The cluster runs outside the sandbox (started
by the supervisor), listens only on a Unix socket in the replay root, and
the harness reaches it through the bind mount. `DATABASE_URL` and `PG*`
variables are never inherited; a fresh database is created per run; the
cluster and the root are removed on every exit path (normal, kill,
exception, timeout, harness failure, PostgreSQL startup failure — the Phase 3
cleanup paths, which are platform-independent and covered by the same tests
on Linux).

Reports record the backend in `environment.isolation` (`bubblewrap`,
`sandbox-exec` or `none`, schema version 2) and the read-only paths in
`environment.networkIsolation`.

## Results on Linux (2026-10-25)

Environment: Debian GNU/Linux 12, kernel 7.0.12-linuxkit aarch64, rustc
1.99.1, Node.js v22.23.4, Bun 1.3.3, bubblewrap 0.8.0, PostgreSQL 15.19.
Container run as an **unprivileged user** with `--security-opt
seccomp=unconfined --security-opt systempaths=unconfined` and no added
capabilities (`scripts/linux-replay-docker.sh`, image
`ci/linux-replay.Dockerfile`).

| Check | Result |
| --- | --- |
| `cargo fmt --check`, `cargo clippy -D warnings` | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **245 passed, 0 failed, 0 skipped** (the same tests as on macOS) |
| Isolation probe (Bun and Node) | HTTP blocked; `/tmp/.s.PGSQL.5432` blocked; decoy socket server outside the root received no connection; write into the harness directory blocked; write to `/tmp` stayed in the private tmpfs |
| Detached grandchild (`setsid`) | did **not** survive the run (it does on macOS) |
| Drizzle fixture | 16/16 cases as expected (5 reproduced, 11 not), control passes |
| `a-kill-after-db-commit --repeat 5` | 5/5 RUNTIME_REPRODUCED |
| Completed generated harness | 5/5 as expected |
| Phase 3 Bun harnesses | all as before (inside the test suite) |
| Release archive (`scripts/package-release.sh`, aarch64-unknown-linux-gnu) | built; the packaged binary analyzes the fixtures |

Negative check, same image under **Docker's default seccomp profile**
(unprivileged user namespaces forbidden; run on an earlier Phase 4 build,
before the final `project.rs` performance fix and one HTML test, neither of
which touches replay or isolation):

| Check | Result |
| --- | --- |
| `commitrift replay fixtures/replay/drizzle-node --allow-execution` | **exit 3**, "replay blocked: bubblewrap cannot create its namespaces here (…)" — nothing executed |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **15 replay tests FAIL** with "replay dependencies required: bubblewrap cannot create namespaces here"; 130 pass; none silently skipped |

## CI (`.github/workflows/ci.yml`)

| Job | Runner | What |
| --- | --- | --- |
| `lint` | ubuntu-24.04 | fmt, clippy |
| `static` | ubuntu-24.04, macos-14 | release build, build without the `replay` feature, all tests (replay tests may print SKIPPED here and are **not** counted as runtime verification), SARIF validation against the OASIS 2.1.0 schema, trace schema validation |
| `replay-linux` | ubuntu-24.04 | **required replay**: `COMMITRIFT_REQUIRE_REPLAY=1`; PostgreSQL, bubblewrap, Node 22, Bun 1.3.3; enables unprivileged user namespaces on the runner (Ubuntu 23.10+ AppArmor restriction); `npm ci` + build of the Drizzle fixture; full test suite; Drizzle replay and the Phase 3 regression via the CLI; JSON Schema validation of the verification and regression reports; asserts `environment.isolation == "bubblewrap"` |

A job that cannot isolate or lacks PostgreSQL fails; it cannot turn green by
skipping. **Status: the workflow has not run on GitHub-hosted runners** (no
remote repository exists). Its replay steps were reproduced locally in the
Debian container above; differences on Ubuntu 24.04 runners (AppArmor, the
PostgreSQL 16 path) are handled in the workflow but unverified.
