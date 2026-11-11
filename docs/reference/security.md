# Security

What Midwrite reads, executes and writes, and what its isolation does
and does not protect against. Nothing here is an independent security
audit: the measures below were reviewed by the people and agents who
implemented them, and are pinned by tests named where it matters.

## Commands that never execute project code

`check`, `trace`, `ir`, `explain`, `report`, `init-replay`, `discover`
(with or without `--model-check`), `model-check` without
`--validate-postgres`/`--verify-source` (including `--fault-mode
in-flight`, `--dpor`, pools), and `schema` without `--migrations`. They
read the files you point them at and their configuration or contracts
files; they never import, evaluate or execute project or package code,
start PostgreSQL, open a socket or make network requests; there is no
telemetry and no account. Test: `static_analysis_never_executes_target_code`
analyzes a file whose top-level code would create a marker file, next to a
replay manifest, and checks the marker never appears.

What static analysis reads, and how:

* TypeScript source of the project, and its `tsconfig.json`,
  `package.json`, `midwrite.json` and contracts file.
* Type annotations (string-literal unions) are followed through relative
  imports and re-exports only — never packages or absolute paths — with a
  depth limit.
* To choose zod 3 or zod 4 semantics, `node_modules/zod/package.json` or
  the nearest `package.json` that declares zod (following `catalog:`
  references), only in directories from the importing file up to the
  project root. Metadata only; when no version is found, every
  version-dependent behavior is refused.
* zod schemas are read from their builder chains and evaluated by the
  model's own functions; `$defaultFn` callbacks, `nanoid()` and
  `crypto.randomUUID()` are recognized from syntax and written package
  contracts, never called; a database factory is recognized from its
  declared type; a declared replica factory's body is not interpreted; loops
  are unrolled in the IR; `request.json()` is a synthesized argument.

`init-replay` writes only into its `--output` directory (inside
`--project`, never the project root itself), executes and installs
nothing, never overwrites without `--force`, and cannot be redirected: every
target directory is resolved before any write and must stay inside the
canonical output directory, files are created with `create_new` (no
following of existing links), and text from a trace is sanitized before it
is interpolated into generated TypeScript or SQL comments
(`interpolated_text_cannot_leave_a_comment`,
`init_replay_never_writes_through_a_symlinked_subdirectory`).
`--emit-scenarios`, `-o`, `--output`, `--cache` and `--write-baseline`
write only where you ask.

## Commands that start PostgreSQL or execute code

| command | executes | authorization |
| --- | --- | --- |
| `model-check --validate-postgres` | a private PostgreSQL cluster and the **model's** SQL; no application code | the flag itself |
| `schema --migrations … ` | the migration SQL, on a private cluster | `--allow-execution` |
| `model-check --verify-source` | the application's original code through the named source harness | `--allow-execution` and a `midwrite-source.json` manifest |
| `replay` | the harness directory you name | `--allow-execution` and a `midwrite-replay.json` manifest; without the flag only the plan is printed (exit 2) |

There is no environment variable or scenario field that authorizes
execution. Nothing is discovered or run because a repository contains a
manifest. `discover` emits `sourceVerification` sections only from the
harness mapping you declared, and running them still needs the flags above
(`generated_scenarios_never_run_application_code_without_authorization`).

### What executing commands enforce

| measure | how |
| --- | --- |
| validated manifest first | unknown fields rejected; the command is argv (no shell anywhere); setup files, analysis paths and the entry point relative, without `..`, canonicalized inside the harness directory; compiled entries must exist (Midwrite compiles and installs nothing); a `MIDWRITE-TODO` marker in a manifest, setup SQL or entry point is refused; source harnesses must say `"network": "deny"`; `limits` are range-checked |
| entry consistency | a source harness entry's selector must equal the actor's selector; declared programs are not eligible |
| disposable database | the only provider is a cluster created by `initdb` in a private 0700 temporary directory, `listen_addresses=''`, Unix socket in that directory, trust authentication for a local test user, a fresh database per run, deleted afterwards. Manifest `url`, `connectionString`, `host`, `port`, `password` are refused without echoing the value; client libraries refuse sockets outside the run directory; no connection string is read from the environment |
| clean environment | cleared and rebuilt (`PATH`, private `HOME`/`TMPDIR`, Midwrite variables, validated manifest `env`). Refused manifest names: `PG*`, `DATABASE*`, `DB_*`, `MIDWRITE*`, `LD_*`, `DYLD_*`, `NODE_*`, `BUN_*`, `PATH`, `HOME`, anything containing `SECRET`, `TOKEN`, `PASSWORD`, `KEY`, `CREDENTIAL` and similar words; values containing `://`. An inherited `PGPASSWORD` is removed |
| OS isolation | macOS `sandbox-exec`: IP networking denied, Unix-socket connections and file writes only inside the run directory. Linux bubblewrap: new user (same uid), network (loopback only), PID, IPC, UTS and cgroup namespaces; read-only view of system directories, the harness project (its Git work tree) and the runtime installation; only the run directory writable; private `/tmp`; `--die-with-parent`. Bubblewrap is probed; if a fresh `/proc` cannot be mounted it refuses rather than bind-mounting the host's (which would expose other processes' environments). Without a backend, execution is **blocked** (exit 3) unless the manifest says `"network": "allow"` **and** `--allow-network` is given (replay only) |
| bounded execution | wall-time deadline from the manifest (`timeoutSeconds` / `timeoutMs`, at most 300 s), then SIGKILL to the process group |
| per-worker rlimits | CPU time 120 s (manifest `limits.cpuSeconds`), file size including logs 64 MiB, open files 1024 (never above the inherited hard limit), no core dumps; `max_connections=40` per cluster |
| cleanup | every exit path (`Drop`): process groups killed, cluster stopped, run directory removed (`--keep-artifacts` or `MIDWRITE_KEEP_ARTIFACTS=1` keeps it). A worker is reaped only after it exited (`waitid(WNOWAIT)`), then its group is killed, so a group id cannot have been reused. A random per-run token in the environment lets every surviving process carrying it be killed after the run (a survivor makes the run inconclusive); on Linux the PID namespace ends with the worker |
| redaction | only environment variable **names** are recorded; harness output truncated (16 KiB per stream); the run directory becomes `<replay-root>`, the harness directory `<harness>`, and `scheme://user:password@` becomes `user:***` |

Every failure of this machinery ends a run `blocked` or `inconclusive`,
never with a verdict. A quiet period is classified as pool starvation or an
application wait cycle only from the pool's or PostgreSQL's own counters; a
worker that misses the deadline makes the run inconclusive, never a
verified deadlock.

### The fault proxy (in-flight and connection-loss runs)

The proxy exists only inside a `--verify-source --allow-execution` run
whose plan contains an in-flight or connection-loss step.

* It listens only on `<run>/px/.s.PGSQL.5432` (directory 0700, removed on
  drop), has no TCP listener or client, and refuses sockets outside the run
  directory; it acts only on connections it accepted itself. Closing a
  client connection takes no address argument and can happen once per
  connection.
* Faults are armed per backend pid (or `application_name`) of a named
  execution; other traffic is forwarded unchanged.
* The wire log records event names and transaction control only. Bind
  parameters, result rows, ordinary SQL text, authentication frames,
  startup parameters other than `application_name` and the cancel key are
  never recorded. Held bytes of a withheld `COMMIT` live in memory only.
* Limits: 8 MiB per frame, 8192 events, 64 connections, 16 MiB held per
  connection.
* The proxy never invents a server answer: every fate is established from
  its own events, `pg_stat_activity` and the rows. A statement on a client
  the driver marked broken is released and recorded, never matched against
  the plan; a worker that died is reported as diverged, never relabeled as a
  connection loss.

The response checkpoint adds one message (`respond`), active only when the
coordinator sets `MIDWRITE_SOURCE_RESPONSE=hold` and only after the
harness entry returned. Replica schedules are refused for original-code
runs and for the lockstep oracle; no replication is ever configured.

## Threat model

Isolation protects against **accidents in a trusted, cooperative
harness**: pointing it at a real database, reaching a real payment or email
service, inheriting credentials, leaving processes or data behind. It is
**not** a sandbox for malicious code. Only run harnesses you have reviewed.

Not protected:

* **Reads.** macOS restricts writes and network, not reads. On Linux the
  project and runtime installation are readable: a secret stored inside the
  project (a `.env` file) never reaches the harness's environment, but its
  code could open the file.
* **Sockets inside visible paths** (Linux): a live Unix socket inside the
  project tree or runtime installation is reachable; macOS restricts socket
  connections to the run directory.
* **Same-user access to the disposable database**: another process of your
  user that finds the private socket during the run could connect.
* **Resources**: memory (`RLIMIT_AS` breaks V8), process count
  (`RLIMIT_NPROC` is per user) and temporary disk are not capped; CPU only
  per worker.
* **macOS detached processes** that leave the process group are found only
  by the token sweep; a worker stuck in a busy loop after the coordinator
  is killed is bounded by its CPU limit.
* **The platforms**: `sandbox-exec` is deprecated by Apple (its
  effectiveness is re-tested by the suite); bubblewrap is as strong as
  unprivileged user namespaces; kernel bugs are out of scope.
* **Harness correctness**: replay trusts that a checkpoint is sent only
  after the named operation completed (`verifyBeforeInjection` lets a case
  check it); several tasks of one worker share memory (the process is the
  boundary).
* **Vendored runtime**: generated harnesses copy the Node client; after
  upgrading Midwrite, copy the new `runtime/node` files. Only the protocol
  version is checked at run time.

Open before a public release: an independent review of the replay crate,
the sandbox profile and the bubblewrap arguments; memory and process caps;
a replacement for `sandbox-exec`.

## Data handling

* Reports include short source excerpts (operation labels, conditions,
  file paths). Treat JSON, SARIF and HTML reports like source code when
  sharing them. First-party original-code evidence can contain absolute
  paths of the analysis machine.
* The discovery report names the project by its directory name and every
  location relative to the project root; emitted scenarios refer to the
  project root by a path relative to the output directory — emit inside the
  repository to avoid recording the location of a private checkout.
* Scenarios, reproduction artifacts and synthesized preconditions contain
  only synthesized values (`name-1`, stable UUID literals, enum members,
  string literals copied from the source's own comparisons, zod-conforming
  values built from the schema's checks); reproduction artifacts carry
  FNV-64 fingerprints of source files, not their contents. Generated values
  render as `gen#g:a.n` or `{"$generated": …}`.
* Synthesized authorization preconditions choose callers for which the
  application's own guards pass; they never remove a guard and are stated
  in every scenario that uses them.
* HTML reports contain no scripts (CSP `default-src 'none'`,
  `script-src 'none'`) and escape every string; tests poison every string
  field of fresh reports.
* Nothing is uploaded anywhere.

## Test-only execution and seams

* The zod differential test runs Node.js on pinned zod packages
  (`fixtures/apps/zod`); the Drizzle SQL probe
  (`fixtures/apps/drizzle-app`) calls `.toSQL()` without a database; the
  defaults probe and lock-matrix ground truth start their own disposable
  clusters. None is part of `discover` or `model-check`.
* The `mutation-testing` feature of `midwrite-model` (deleting a kind of
  access from footprints) is enabled only for that crate's own tests; the
  release CLI does not include it. `MIDWRITE_DPOR_PROFILE=1` prints a
  profile and changes no result.
* The Linux container mounts the repository read-only and writes only its
  output directory; building the image and `npm ci` download packages, the
  tests themselves run without network.

## Evaluation privacy

Two corpus projects (school-ledger and unnamed-ai-project) are private.
Reports and these documents contain only aggregate counts and generic
categories about them — no paths, selectors, identifiers, source text or
SQL parameters. They are analyzed statically from read-only copies of
pinned revisions (`git archive` into a scratch directory, without
`node_modules`); their working trees are never modified, and intermediate
full reports go to a scratch directory outside the repository. No corpus
project has ever been executed. Contributors must keep it that way: never
commit names of their files or functions.
