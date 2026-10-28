# Security

What CommitRift reads, executes and writes, and what its isolation does
and does not protect against. Per-phase reviews:
[Phase 3](../phase3/SECURITY.md), [Phase 4](../phase4/SECURITY-REVIEW.md),
[Phase 4 Linux isolation](../phase4/LINUX-ISOLATION.md),
[Phase 7](../phase7/SECURITY.md), [Phase 8](../phase8/SECURITY.md),
[Phase 9](../phase9/SECURITY.md), [Phase 10](../phase10/SECURITY.md),
[Phase 11](../phase11/SECURITY.md), [Phase 12](../phase12/SECURITY.md),
[Phase 13](../phase13/SECURITY.md).

## Commands that never execute project code

`check`, `trace`, `ir`, `explain`, `report`, `init-replay`, `discover`
(with or without `--model-check`), `model-check` without
`--validate-postgres`/`--verify-source`, and `schema` without
`--migrations`. They read the files you point them at and their
configuration or contracts files; they never import, evaluate or execute
project or package code, make no network requests, have no telemetry and
need no account. A declared replica factory's body is not interpreted, and
a package contract never runs the package.

`init-replay` writes only into its `--output` directory (inside
`--project`), executes and installs nothing, and never overwrites files
without `--force`. `--emit-scenarios`, `-o`, `--output`, `--cache` and
`--write-baseline` write only where you ask.

## Commands that start PostgreSQL or execute code

| command | executes | authorization |
| --- | --- | --- |
| `model-check --validate-postgres` | a private PostgreSQL cluster and the **model's** SQL; no application code | the flag itself |
| `schema --migrations … ` | the migration SQL, on a private cluster | `--allow-execution` |
| `model-check --verify-source` | the application's original code through the named source harness | `--allow-execution` and a `commitrift-source.json` manifest |
| `replay` | the harness directory you name | `--allow-execution` and a `commitrift-replay.json` manifest; without the flag only the plan is printed |

Guarantees for executing commands (Phases 3–13):

* **Disposable database.** A private cluster in a temporary directory,
  Unix socket only (`listen_addresses=''`), deleted afterwards. External
  database URLs and credential-like environment variables in a manifest are
  refused. No production database was contacted in any phase.
* **Clean environment.** The environment is cleared before the harness
  runs.
* **OS isolation.** macOS `sandbox-exec` denies IP networking, other Unix
  sockets and file writes outside the run's temporary directory. Linux
  bubblewrap runs workers in new user, network (loopback only), PID, IPC,
  UTS and cgroup namespaces with a minimal read-only filesystem view.
  Without an isolation backend, execution is **blocked** (exit 3) unless
  both the manifest (`"network": "allow"`) and the command line
  (`--allow-network`, replay only) allow the network; source harnesses
  require `"network": "deny"`.
* **Process control.** Per-worker rlimits, a run-token sweep of detached
  processes, worker self-exit, process-group kills (Phase 8).
* **Manifest checks.** Files must be inside the harness directory; a
  harness with a `COMMITRIFT-TODO` marker is refused.
* **Protocol additions are narrow.** The Phase 13 `respond` message is
  active only when the coordinator sets `COMMITRIFT_SOURCE_RESPONSE=hold`
  and only after the harness entry returned; no network listener is
  opened. Replica schedules are refused for original-code runs.

## Threat model

Isolation protects against **accidents in a trusted, cooperative
harness**. It is **not** a sandbox for malicious code: on Linux the harness
project and runtime installation are readable inside the sandbox; on macOS
processes leaving the harness's process group could outlive a run (swept
since Phase 8); there is no CPU, memory or temporary-disk cap beyond
per-worker rlimits. Only run harnesses you have reviewed.

## Data handling

* Reports include short source excerpts (operation labels, conditions,
  file paths). Treat JSON, SARIF and HTML reports like source code when
  sharing them.
* HTML reports contain no scripts (`script-src 'none'`) and escape every
  string; tests poison every string field of fresh reports.
* Phase 9 wire logs contain no parameter values, rows, ordinary SQL or
  authentication data.
* Nothing is uploaded anywhere.

## Evaluation privacy

Two corpus projects (school-ledger and unnamed-ai-project) are private.
Phase reports and these documents contain only aggregate counts and generic
categories about them — no paths, selectors, identifiers or SQL
parameters — and they were analyzed statically from read-only copies,
never executed or modified ([Phase 13 security](../phase13/SECURITY.md)).
Contributors must keep it that way: never commit names of their files or
functions.
