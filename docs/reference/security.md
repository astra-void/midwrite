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
| `schema --migrations … ` | the migration SQL, through `psql` (not isolated), as an ordinary role on a private cluster | `--allow-execution` |
| `model-check --verify-source` | the application's original code through the named source harness | `--allow-execution` and a `midwrite-source.json` manifest |
| `replay` | the harness directory you name | `--allow-execution` and a `midwrite-replay.json` manifest; without the flag only the plan is printed (exit 2). The library refuses as well (`ReplayOptions::allow_execution`), so no other caller can skip the flag |

There is no environment variable or scenario field that authorizes
execution. Nothing is discovered or run because a repository contains a
manifest. `discover` emits `sourceVerification` sections only from the
harness mapping you declared, and running them still needs the flags above
(`generated_scenarios_never_run_application_code_without_authorization`).

### What executing commands enforce

| measure | how |
| --- | --- |
| validated manifest first | unknown fields rejected; the command is argv (no shell anywhere); setup files, analysis paths and the entry point relative, without `..`, canonicalized inside the harness directory; compiled entries must exist (Midwrite compiles and installs nothing); a `MIDWRITE-TODO` marker in a manifest, setup SQL or entry point is refused; source harnesses must say `"network": "deny"`; `limits` are range-checked; a source-harness manifest must be named `midwrite-source.json` or `midwrite-source-<variant>.json`; SQL files (setup, schema, migrations) and observation queries containing a `psql` meta-command (`\!`, `\o |…`, `\copy`, `\i`: a backslash outside strings and comments) or `COPY … PROGRAM` are refused before anything starts, because `psql` runs outside the isolation |
| entry consistency | a source harness entry's selector must equal the actor's selector; declared programs are not eligible |
| disposable database | the only provider is a cluster created by `initdb` in a private 0700 temporary directory, `listen_addresses=''`, Unix socket in that directory, a fresh database per run, deleted afterwards. Two roles: harnesses and project SQL (setup files, migrations, observations) use `midwrite_app`, an ordinary role that owns each database, trusted without a password; the superuser `midwrite` is used only by Midwrite and needs a random per-cluster password that is never in a harness' environment. So a harness cannot make the unsandboxed server run a program or write a file (`COPY … PROGRAM`, `COPY … TO '<file>'`). Manifest `url`, `connectionString`, `host`, `port`, `password` are refused without echoing the value; client libraries refuse sockets outside the run directory; no connection string is read from the environment |
| clean environment | cleared and rebuilt (`PATH`, private `HOME`/`TMPDIR`, Midwrite variables, validated manifest `env`). Refused manifest names: `PG*`, `DATABASE*`, `DB_*`, `MIDWRITE*`, `LD_*`, `DYLD_*`, `NODE_*`, `BUN_*`, `PATH`, `HOME`, anything containing `SECRET`, `TOKEN`, `PASSWORD`, `KEY`, `CREDENTIAL` and similar words; values containing `://`. An inherited `PGPASSWORD` is removed |
| OS isolation | macOS `sandbox-exec`: IP networking denied, Unix-socket connections and file writes only inside the run directory. Linux bubblewrap: new user (same uid), network (loopback only), PID, IPC, UTS and cgroup namespaces; read-only view of system directories, the harness project (its Git work tree) and the runtime installation, each narrowed so that it never exposes the home directory (a runtime in `~/bin` mounts `~/bin`, a dotfiles repository in `~` mounts only the harness directory); only the run directory writable; private `/tmp`; `--die-with-parent`. Bubblewrap is probed; if a fresh `/proc` cannot be mounted it refuses rather than bind-mounting the host's (which would expose other processes' environments). Without a backend, execution is **blocked** (exit 3) unless the manifest says `"network": "allow"` **and** `--allow-network` is given (replay only) |
| bounded execution | wall-time deadline from the manifest (`timeoutSeconds` / `timeoutMs`, at most 300 s), then SIGKILL to the process group |
| per-worker rlimits | for every harness process, of `replay` and of source runs: CPU time 120 s (source manifests: `limits.cpuSeconds`), file size including logs 64 MiB, open files 1024 (never above the inherited hard limit), no core dumps; `max_connections=40` per cluster |
| cleanup | every exit path (`Drop`): process groups killed, cluster stopped, run directory removed (`--keep-artifacts` or `MIDWRITE_KEEP_ARTIFACTS=1` keeps it). A worker's group is killed after every exit, also a normal one, so a background process it left in its group dies with it. The worker is reaped only after that (`waitid(WNOWAIT)` first), so the group id cannot have been reused. A random token in each worker's environment lets processes that left the group be found afterwards and killed; such a process makes the run inconclusive. Finding them reads other processes' environments, which works on Linux (`/proc/<pid>/environ`) but not on macOS 27 (see below); on Linux the PID namespace also ends with the worker |
| cancellation | SIGINT, SIGTERM and SIGHUP are caught while a cluster or harness runs: the first one ends the run like its deadline (workers killed, cluster stopped, run directory removed, exit `blocked`); a second one exits at once. Without this, Ctrl-C left the `pg_ctl` daemon running with its shared memory |
| program lookup | `initdb`, `pg_ctl`, `postgres`, `psql`, `pg_config` and `bwrap` are looked up only in absolute `PATH` entries (a `.` or `node_modules/.bin` entry would let the analyzed repository supply them); `/usr/bin/bwrap` is preferred to any `bwrap` on `PATH`; `git` (for the report's revision) runs with `core.fsmonitor=false` |
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
* **SQL as code**: `psql` and the PostgreSQL server run outside the
  isolation. Shell escapes are refused and project SQL runs as an ordinary
  role, but SQL files are still code: they can use all the time and disk
  the limits allow.
* **Resources**: memory (`RLIMIT_AS` breaks V8), process count
  (`RLIMIT_NPROC` is per user) and temporary disk are not capped; CPU only
  per worker.
* **macOS detached processes.** A process that leaves the worker's process
  group (`setsid`, `detached: true`) is not found on macOS 27: the kernel no
  longer returns another process' environment (`ps -E`, `KERN_PROCARGS2`),
  so the token sweep sees nothing. It keeps the sandbox profile and its CPU
  limit, but may outlive the run. A harness can also ask `launchd` to
  start programs (`open`, `osascript`); those run outside the sandbox.
* **Total disk use.** Each file is capped at 64 MiB, but the number of files
  in the run directory is not.
* **`kill -9` of Midwrite** cannot be caught: the cluster and run directory
  stay behind (on Linux the harness dies with its parent).
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

Open before a public release: an independent (human) review of the replay
crate, the sandbox profile and the bubblewrap arguments; memory and process
caps; a replacement for `sandbox-exec`; a way to find detached processes on
macOS.

### Review of 2026-11-12 (DEV-163)

An AI review of every execution path, the isolation backends, resource
limits and cleanup (not the independent review above). Findings, by
severity under the threat model above:

| severity | finding | disposition |
| --- | --- | --- |
| high | `replay` harness processes ran without the rlimits, group kill after a normal exit and survivor sweep that the docs listed (only source runs had them) | fixed: shared with source runs; `supervisor` tests |
| medium | harnesses connected as the cluster's superuser (trust authentication): `COPY … PROGRAM` or `COPY … TO '<file>'` through the unsandboxed server gets around the isolation | fixed: `midwrite_app` role, password-only superuser; `cluster_roles` test |
| medium | `psql` meta-commands (`\!`) in setup, migration or observation SQL run shell commands outside the isolation, also for `schema --migrations` | fixed: refused at load; `postgres` unit tests, `setup_sql_cannot_run_shell_commands_even_when_authorized`, `reading_a_schema_is_static_and_comparing_it_needs_authorization` |
| medium | Ctrl-C/SIGTERM left the PostgreSQL daemon, its shared memory and the run directory behind (31 leaked segments filled macOS's limit of 32 on the reviewing machine) | fixed: signal handler, cancellation in every run loop |
| medium | Linux: a runtime in `~/bin` or a Git work tree at `~` mounted the whole home directory read-only (sockets in it reachable) | fixed: `mounts_never_expose_the_home_directory` |
| medium | macOS 27: the token sweep cannot read process environments, so detached processes are not found | documented (above); Linux test only |
| low | relative `PATH` entries could supply `initdb`/`psql`/`bwrap`; `bwrap` preferred from `PATH` | fixed |
| low | `replay`'s consent check lived only in the CLI | fixed: `ReplayOptions::allow_execution` |
| low | a scenario's `sourceVerification.harness` could name any JSON file | fixed: file name required |
| low | `git status` in the harness ran `core.fsmonitor` from the repository's configuration | fixed: `-c core.fsmonitor=false` |
| low | macOS: `launchd`-started programs leave the sandbox | documented (above) |
| info | `--allow-execution` is accepted and ignored by commands that do not execute | unchanged |

No consent bypass was found: no command executes harness code, applies
project SQL or starts PostgreSQL without its flag, and no environment
variable, scenario field or discovered manifest authorizes execution
(pinned by `a_valid_harness_is_never_run_without_authorization` and the
tests named above).

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
