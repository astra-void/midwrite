# Security review of replay, isolation and harness generation (Phase 4)

Scope: `crates/commitrift-replay` (supervisor, manifest, isolation, safety,
PostgreSQL), `runtime/node`, `crates/commitrift-cli/src/init_replay.rs`.
Reviewer: the Phase 4 implementer (an AI coding agent), by reading the code
and adding tests. **This is not an independent security audit**, and nothing
here establishes that replay contains deliberately malicious code.

The trust model is unchanged from [`../phase3/SECURITY.md`](../phase3/SECURITY.md):
a harness is trusted code the user authorizes; isolation protects a
cooperative harness against accidents (real databases, real services,
inherited credentials, leftovers). Static commands (`check`, `trace`, `ir`,
`explain`, `report`, `init-replay`) never execute project code.

## Findings and changes

| # | Area | Finding | Resolution |
| --- | --- | --- | --- |
| 1 | Generator | Labels, names and paths from a trace file were interpolated into `//` comments of `harness.ts` and `--` comments of `schema.sql`; a newline in a (tampered) trace would have become TypeScript or SQL that replay later executes. | **Fixed**: control characters are replaced before interpolation; quotes are neutralized in string contexts; scenario names are identifiers. Test `interpolated_text_cannot_leave_a_comment`. |
| 2 | Generator | A pre-existing symbolic link for a subdirectory of `--output` (`commitrift-runtime -> /elsewhere`) would have redirected writes outside the output directory. | **Fixed**: every target directory is resolved before any write and must stay inside the canonical output directory; files are created with `create_new` (no following of existing links); a refusal leaves no partial skeleton. Test `init_replay_never_writes_through_a_symlinked_subdirectory`. |
| 3 | Manifest | Only `NODE_OPTIONS` among Node.js loader variables was refused. | **Changed**: every `NODE_*` name is refused (e.g. `NODE_PATH` could redirect module resolution). Test `credential_and_database_variables_are_refused`. |
| 4 | Runtime | An injected exception wrapped by a library (Drizzle's `DrizzleQueryError`) was reported as an ordinary error. | **Fixed**: the runtime follows `cause` chains; reports record `injectedFaultSurfaced`. |
| 5 | Isolation (Linux) | Phase 3 had no Linux isolation; running required disabling all isolation. | **Implemented** with bubblewrap, probed and fail-closed (below). |
| 6 | Isolation (macOS) | A harness process that leaves its process group (`setsid`) outlives the run (Phase 3 limitation). | **Unchanged, now demonstrated**: `detached_grandchildren_do_not_survive_on_linux` observes one survivor on macOS (then kills it). On Linux the PID namespace ends with the harness (see the test result in `FINAL-REPORT.md`). |
| 7 | Placeholders | A generated, unfinished harness could have run with nonsense observations. | Replay refuses manifests, setup SQL and compiled entry points containing `COMMITRIFT-TODO`. |

## Checklist

| Item | State |
| --- | --- |
| Child lifecycle | One process group per harness run; SIGKILL to the group on kill, timeout, protocol violation and failed pre-checks; reaped with a deadline. Linux adds `--die-with-parent` and a PID namespace. |
| Process-group termination | Kill only while the harness waits at the checkpoint (no race). Exit status of a killed run: signal 9 on both platforms (bubblewrap is the direct child and is in the killed group). |
| Environment filtering | Cleared and rebuilt (`PATH`, private `HOME`/`TMPDIR`, replay variables, validated manifest `env`). Refused names: `PG*`, `DATABASE*`, `DB_*`, `COMMITRIFT*`, `CRASHPROOF*`, `LD_*`, `DYLD_*`, `NODE_*`, `BUN_*`, `PATH`, `HOME`, anything containing `SECRET`, `TOKEN`, `PASSWORD`, `KEY`, `AUTH`, `URL`, `DSN`, …; values with `://`. Tested with a planted `DATABASE_URL`, a cloud key and a custom secret. |
| Filesystem isolation | macOS: writes denied outside the replay root (reads allowed). Linux: only system directories, the harness project (its Git work tree) and the runtime's installation are visible, read-only; the replay root is the only writable host directory; private `/tmp`. Tested: a write into the harness directory is blocked on both; a write to `/tmp` never reaches the host. |
| Network isolation | macOS: IP denied, Unix sockets only inside the replay root. Linux: network namespace with loopback only; host sockets outside the visible directories do not exist inside. Tested on both with an HTTP fetch, `/tmp/.s.PGSQL.5432` and a decoy Unix-socket server outside the replay root (no connection accepted). |
| Unix socket permissions | Protocol and PostgreSQL sockets live in the 0700 replay root. Another process **of the same user** that finds the path during the run could still connect (unchanged). |
| Manifest path validation | Setup files and analysis paths: relative, no `..`, canonicalized inside the harness directory; `analysis.root` only `.`/`..` segments resolving to the harness directory or an ancestor; runtime entry point relative without `..`. A symlinked entry point is followed (the harness is trusted). |
| Symlinks / traversal (generator) | See findings 1–2; output must be inside `--project`; the project root itself is refused. |
| Database credentials | None exist: trust authentication for a local test user on a private socket; `url`/`host`/`port`/`password` fields refused without echoing values; the client libraries refuse sockets outside the replay root; `replayPoolConfig()` cannot be overridden to another host. |
| Temporary directories | 0700, short paths (socket length), unique per invocation (pid, time, counter), removed on drop unless `--keep-artifacts`. |
| Concurrent replays | Independent roots and clusters; the test suite runs replays in parallel on both platforms. |
| Report redaction | Replay root path and `scheme://user:password@` credentials redacted from logs and errors; environment variable **names** only. |
| Command injection | No shell anywhere; manifest `command` is argv; bubblewrap arguments are separate argv entries built from canonical paths; the sandbox-exec profile escapes quotes and backslashes in the root path. |
| Generated code | No execution, no installation, no network; inputs sanitized (finding 1). |

## Linux isolation: what it does not protect

* **Same-user reads inside visible paths**: the project directory and the
  runtime installation are readable; secrets stored *inside the project*
  (e.g. a `.env` file) are readable by the harness. The environment of the
  harness never contains them, but its code could open the file.
* **Sockets inside visible paths**: a live Unix socket inside the project
  tree or the runtime installation is reachable (read-only bind mounts do
  not prevent `connect`). macOS does not have this gap (socket connections
  are restricted to the replay root).
* **Kernel and bubblewrap**: the isolation is as strong as unprivileged user
  namespaces and bubblewrap; kernel bugs are out of scope.
* **Resource exhaustion** inside the deadline (CPU, memory, disk in the
  replay root): not limited.
* **Weakened runner settings**: enabling unprivileged user namespaces on a
  CI runner (Ubuntu's AppArmor restriction) is a property of that disposable
  machine, not something CommitRift changes.

## Remaining concerns before a public beta

1. An independent review of `commitrift-replay`, the sandbox-exec profile and
   the bubblewrap arguments.
2. Resource limits (cgroups or `setrlimit`) for harness processes.
3. macOS: processes that leave the process group.
4. `sandbox-exec` is deprecated by Apple; a replacement needs evaluation.
5. Generated harnesses vendor the client: after upgrading CommitRift, users
   must copy the new `runtime/node` files into `commitrift-runtime/`; nothing checks the vendored
   copy's version at runtime beyond the protocol version.
