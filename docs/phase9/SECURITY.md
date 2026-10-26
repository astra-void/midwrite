# Phase 9 security

Phase 9 adds a component that sits on the database connection of an
authorized worker. Everything in `docs/phase8/SECURITY.md` still holds; this
note records what the proxy, the multi-task worker and the fault modes add,
and what is **not** enforced.

## Execution authorization

* Static analysis (`commitrift check`) and model-only exploration
  (`commitrift model-check scenario.json`, including `--fault-mode in-flight`,
  `--dpor`, pools) never execute application code, never start PostgreSQL and
  never open a socket. Test: `model_only_runs_explain_both_worlds_without_running_anything`.
* `--verify-source` without `--allow-execution` is *blocked* before anything is
  started (`executing_the_application_still_needs_the_explicit_flag`); the
  in-flight modes add no way around it. There is no environment variable or
  scenario field that authorizes execution.
* The proxy exists only inside a `--verify-source --allow-execution` run whose
  plan contains an in-flight step.

## What the proxy can reach

| property | enforcement |
| --- | --- |
| only the run's disposable cluster | `FaultProxy::start` refuses sockets outside the run directory; the coordinator's `check_database_target` checks the socket handed to the worker; the harness client (`sourcePoolConfig`) refuses any socket outside `COMMITRIFT_REPLAY_ROOT` |
| never production | the cluster is created by the coordinator in a private temporary directory (`initdb`, trust auth over a Unix socket, `listen_addresses=''`); no connection string is read from the environment (`DATABASE_URL`, `PG*` are never consulted; the worker's environment is built from scratch) |
| no network | the proxy has no TCP listener and no TCP client; workers run under the OS isolation of Phase 3/4 (sandbox-exec: IP denied, Unix sockets only in the run directory; bubblewrap on Linux: no network namespace) |
| local-only transport | `<run>/px/.s.PGSQL.5432`, directory mode 0700 (`create_private_dir`; test `the_proxy_socket_is_private_and_removed`), removed on drop |
| only the controlled connections | faults are armed per backend pid (or `application_name`) of a named execution; other traffic is forwarded unchanged (`only_the_armed_connection_is_touched_and_other_traffic_is_unchanged`) |

## Protocol data

* Authentication: the cluster uses `trust`; if a password exchange occurred the
  `p` frames are forwarded and **never recorded** (message log says
  "authentication data not recorded"), startup parameters other than
  `application_name` are not kept, and the cancel key of `BackendKeyData` is
  never stored (only the pid).
* Payloads: Bind parameters, result rows (`D`, `T`) and the SQL of ordinary
  statements are never recorded; transaction control (`BEGIN`, `COMMIT`,
  `ROLLBACK`) is. Test `the_wire_log_names_events_but_carries_no_payloads`
  checks that a request id, an amount and the text `insert` do not appear in
  the report's wire log; `a_dropped_response_leaves_a_committed_row…` checks the
  proxy's own event log for a parameter value.
* Reports pass through `safety::redact` for worker stderr (replay root paths,
  URL credentials); the wire log is built redacted.
* The held bytes of a withheld COMMIT live in memory only until delivered or
  discarded.

## Limits

| limit | value | enforced where |
| --- | --- | --- |
| frame size | 8 MiB; larger → connection unsupported | `Framer` |
| events kept | 8192; the rest counted | `Shared::event` |
| proxy connections | 64 | `accept_loop` |
| held data per connection | 16 MiB | `forward_client_frame` |
| per-worker rlimits (CPU, file size, files) | Phase 8 values | `apply_limits` |
| run deadline | the manifest's `timeoutMs` | coordinator |
| state-space memory | `maxMemoryBytes` against the compact table (arena + interner + edges) | explorer |
| worker processes | one process group per execution; every worker killed on every exit path (`Drop`) | coordinator |
| temporary disk | the run directory is removed on every exit path (`ReplayRoot`); a size cap is **not** enforced |
| number of database connections | the cluster's `max_connections = 40`; the proxy adds none of its own |

Not enforced: a memory cap on the worker or on the coordinator's own heap
beyond rlimits; a cap on temporary disk; protection against a *hostile*
harness (the isolation is for an authorized, cooperative one, as before).

## Cleanup

`no_worker_or_proxy_outlives_a_run` checks the process table after a run with a
held COMMIT delivered after the kill. The proxy threads are joined on `stop()`
(called on drop), which also closes every kept server connection; the cluster is
stopped and the directory removed after it (field order). A proxy or coordinator
failure ends the run INCONCLUSIVE/blocked, never with a verdict.

## Multi-task workers

Several tasks in one worker process share its memory and its pool; a hostile
task could read another task's data — irrelevant for an authorized harness, and
the isolation boundary is the process, as before. The harness-provided task
context is a measurement aid, not a security boundary.
