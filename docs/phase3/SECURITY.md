# Security of runtime replay

`commitrift check`, `trace`, `ir`, `explain` and `report` never execute,
import or evaluate project code, make no network requests and have no
telemetry. This is tested: `static_analysis_never_executes_target_code`
analyzes a file whose top-level code would create a marker file, with a
replay manifest next to it, and checks that the marker never appears.

`commitrift replay` **does execute code**: the harness in the directory you
name. This document says what it protects against and what it does not.

## Trust model

* A harness is **trusted code**, to the extent that you authorize it. Replay
  never discovers harnesses, never runs anything because a repository
  contains a manifest, and never runs without `--allow-execution`; without it
  you get the plan (command, working directory, cases) and exit status 2.
* The isolation below protects against **accidents** in a cooperative
  harness: pointing it at a real database, reaching a real payment or email
  service, inheriting credentials, leaving processes or data behind.
* **It is not a sandbox for malicious code.** A hostile harness runs with
  your user's privileges (minus the restrictions below, where available), can
  read any file you can read, can consume CPU and memory, and can exploit any
  weakness of the platform sandbox. Do not replay code you would not run
  yourself.

## What replay enforces

| Measure | How | Tested by |
| --- | --- | --- |
| Explicit opt-in | `--allow-execution` required; no discovery, one named directory | `replay_requires_explicit_authorization`, `a_directory_without_a_manifest_is_not_a_harness` |
| Validated manifest before anything runs | Unknown fields rejected; command run without a shell; setup files and analysis paths must stay inside the harness directory | manifest unit validation, `production_looking_database_targets_are_refused_without_echoing_secrets` |
| No existing database | The only provider is `ephemeral-postgres`: a cluster created by `initdb` in a private 0700 temporary directory, `listen_addresses=''` (no TCP), Unix socket in that directory, a fresh database per run. `url`, `connectionString`, `host`, `port`, `password` are refused, and the refusal does not echo the value | same test; `database_targets_outside_the_root_are_refused` |
| Production-looking settings cannot be injected | Manifest `env` names that look like database settings, loader settings, replay internals or credentials are refused, as are values containing `://` | same test |
| No inherited environment | The harness environment is cleared and rebuilt (`PATH`, a private `HOME`/`TMPDIR`, replay variables, validated manifest `env`). A `DATABASE_URL`, cloud keys or tokens in your shell never reach it | `the_harness_sees_no_inherited_secrets_and_no_network` |
| Client-side check | `replayDatabase()` refuses a socket outside `COMMITRIFT_REPLAY_ROOT` | (client code) |
| Network isolation (macOS) | `sandbox-exec` profile: deny IP traffic in and out, deny Unix-socket connections outside the replay directory (this blocks e.g. a local PostgreSQL at `/tmp/.s.PGSQL.5432`), deny file writes outside it | same test: HTTP fetch blocked, a local server's socket blocked, a write to `/tmp` blocked |
| Fail closed without isolation | On platforms without `sandbox-exec`, replay refuses to run unless the manifest declares `"network": "allow"` **and** `--allow-network` is given; the report then says so | (Linux path not exercised in this environment) |
| Bounded execution | Per-process deadline (manifest `timeoutSeconds`, max 300); the harness runs in its own process group, killed with SIGKILL on timeout | `timeouts_kill_the_harness_and_clean_up` |
| Cleanup | The cluster is stopped and the temporary directory deleted on every exit path (`Drop`), each run's database dropped; `--keep-artifacts` keeps them for inspection | same test: no leftover directory, no surviving process |
| No secrets in reports | Only environment variable **names** are recorded; harness output is truncated to 16 KiB per stream, the temporary path is replaced by `<replay-root>`, and `scheme://user:password@` becomes `user:***` | `redaction_removes_passwords_and_temporary_paths`, the isolation test checks that a planted secret is absent from the report |

## What it does not protect

* Reading files: the sandbox profile restricts writes and network, not reads.
* Resource exhaustion inside the deadline (CPU, memory, disk inside the
  temporary directory).
* Platforms without `sandbox-exec`: with `--allow-network` there is no OS
  isolation at all; only the environment and database measures apply.
  Linux namespaces are not implemented.
* `sandbox-exec` is deprecated by Apple and could change behavior; its
  effectiveness is checked by the test above on each run of the suite.
* Processes a harness detaches from its process group (`setsid`) can outlive
  the deadline.
* PostgreSQL itself: the cluster uses trust authentication for a local user,
  reachable only through its private socket; another process of your user
  that finds the socket during the run could connect to the disposable
  database.
* The harness' own correctness: replay trusts that a checkpoint is sent only
  after the operation it names completed. `verifyBeforeInjection` lets a case
  check this independently (for example that a write is visible from another
  session, i.e. committed); the first-party fixtures do.

## Corpus projects

External corpus projects are only analyzed statically. Replay was never run
on them; the midday result in `REPRODUCTION-RESULTS.md` comes from a
first-party re-write of the code shape (`fixtures/replay/invoice-pattern/`).
