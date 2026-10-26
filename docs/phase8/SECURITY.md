# Security and resource isolation (Phase 8)

Phase 8 adds process kills, restarts, external-effect doubles, a catalog
reader and a second driver. The Phase 4/7 model is kept and strengthened;
nothing was weakened. It is not a sandbox against deliberately malicious
code and has not been independently audited.

## Authorization

* `commitrift check`, `trace`, `ir`, `model-check` (without flags) and
  `commitrift schema` without `--migrations` never execute anything. The
  schema extractor reads the module's syntax only.
* Original code runs only with `model-check --verify-source
  --allow-execution`; migrations run only with `commitrift schema
  --migrations ... --allow-execution` (tests:
  `crash_verification_never_runs_without_the_explicit_authorization`,
  `reading_a_schema_is_static_and_comparing_it_needs_authorization`).
* Harness manifests must deny the network, name only files inside the
  harness directory and contain no `COMMITRIFT-TODO`; new manifest
  `limits` are range-checked.
* External projects are not executed: saas-starter remains statically
  analyzed and model-checked only.

## Resource limits per worker (`setrlimit` before exec, inherited)

| limit | default | enforced on |
| --- | --- | --- |
| CPU time (`RLIMIT_CPU`) | 120 s (manifest `limits.cpuSeconds`) | macOS, Linux (test: a spinning worker is stopped by a 2 s limit) |
| file size incl. stdout/stderr logs (`RLIMIT_FSIZE`) | 64 MiB | macOS, Linux |
| open files (`RLIMIT_NOFILE`) | 1024 (never raised above the inherited hard limit) | macOS, Linux |
| core dumps | 0 | macOS, Linux |
| wall time | manifest `timeoutMs` (≤ 300 s) | coordinator deadline, then SIGKILL |
| PostgreSQL connections | `max_connections=40` per disposable cluster | server |
| memory | **not limited** — `RLIMIT_AS` breaks V8 (it reserves far more virtual memory than it uses); no cgroup is created | disclosed |
| process count | **not limited** — `RLIMIT_NPROC` is per user, not per worker; the group kill and the token sweep bound what survives | disclosed |
| temporary disk | the private run directory (removed afterwards); no quota | disclosed |

## Process cleanup

| case | mechanism | test |
| --- | --- | --- |
| normal end | the worker reports `done`; it is reaped only after it exited (checked without reaping, `waitid(WNOWAIT)`), then its process group is killed, then it is reaped — so the group id cannot have been reused | lifecycle tests |
| planned crash | SIGKILL to the process group, `waitpid`, PostgreSQL session termination observed | `a_planned_crash_kills_the_worker_with_the_next_operation_never_sent` |
| restart that fails to start | reported, run inconclusive | `a_retry_that_fails_to_start_is_inconclusive` |
| hung worker | deadline → SIGKILL | `a_hung_worker_is_killed_at_the_deadline` |
| CPU-bound worker | `RLIMIT_CPU` | `a_cpu_burning_worker_is_stopped_by_its_cpu_limit` |
| detached grandchild (`setsid`) | Linux: the worker's PID namespace (bubblewrap) ends with it and the kernel kills the grandchild. Both platforms: random per-run token in the environment; after the run every process of the user carrying it is killed and the sweep is repeated (a survivor makes the run inconclusive) — on macOS this sweep is what kills it | `a_detached_grandchild_does_not_survive_the_run` (no process with the grandchild's marker survives; on macOS, the sweep killed its pid) |
| coordinator panic | `Drop` kills every worker | (by construction) |
| coordinator killed | workers exit when the coordination socket closes; a worker stuck in a busy loop is not stopped by this (its CPU limit and, on Linux, bubblewrap's `--die-with-parent`/PID namespace bound it) | disclosed |
| PostgreSQL failure | setup failures are `blocked`; a statement that never completes makes the run inconclusive at the deadline | Phase 7 tests |
| lock timeout | a lock wait the model does not predict diverges; one it predicts is checked by `pg_stat_activity` | Phase 7/8 tests |
| sandbox failure | no isolation backend → `blocked` (never runs unisolated) | Phase 4 tests |

## Environment

Workers get an environment built from scratch; the postgres-js URL is
generated (`postgres:///run_db?application_name=...` with
`PGHOST/PGPORT/PGUSER` for the private socket) and any inherited
`PGPASSWORD` is removed; connection strings and secrets are never printed
(Phase 4 redaction applies to new messages). Disposable clusters listen on
a Unix socket inside the private run directory only; the run directory is
removed on every exit path. Effects reach coordinator-hosted test doubles;
workers have no network (macOS `sandbox-exec`, Linux bubblewrap).

Changing `client_connection_check_interval` affects only the disposable
cluster of a crash plan and is reported.
