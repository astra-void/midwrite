# Security of source verification

Source verification runs application code. It reuses the Phase 4–6
replay safety model ([`../phase4/SECURITY-REVIEW.md`](../phase4/SECURITY-REVIEW.md))
and adds nothing that weakens it. The isolation is a guard rail for an
*authorized, cooperative* harness, not a sandbox for hostile code.

| Requirement | How it is met | Tested by |
| --- | --- | --- |
| explicit authorization | nothing runs without `--verify-source` **and** `--allow-execution`; `--validate-postgres` never runs application code | `application_code_never_runs_without_the_explicit_authorization`, `execution_needs_the_explicit_authorization` |
| approved harness | a `commitrift.source-harness` manifest in the harness directory; files must lie inside it; `COMMITRIFT-TODO` markers refused; the compiled entry must exist (nothing is compiled or installed by CommitRift) | `manifests_must_deny_the_network_and_name_existing_files` |
| entry consistency | the harness entry's selector must equal the actor's selector; declared programs are refused | `declared_programs_are_not_eligible_for_source_verification` |
| disposable database | private PostgreSQL cluster (`listen_addresses=''`, Unix socket inside the private run directory, trust auth for a local test user), fresh per run, removed afterwards; the database target is checked to lie inside the run directory | replay safety tests (Phase 3/4), reused unchanged |
| no production credentials | the worker environment is built from scratch (`PATH`, `HOME`/`TMPDIR` inside the run directory, `COMMITRIFT_*` variables only); `sourcePoolConfig` never reads `DATABASE_URL`/`PG*` and refuses sockets outside the run directory | runtime client code; Phase 4 tests for the shared mechanism |
| no network | manifests must say `"network": "deny"` (there is no opt-out for source verification); workers run under `sandbox-exec` (macOS: IP denied, Unix sockets and writes limited to the run directory) or bubblewrap (Linux: new network namespace, minimal filesystem view) | Phase 4 isolation tests; the Linux run used bubblewrap |
| bounded execution | manifest `timeoutMs` (1–300 000) bounds the whole run; workers are killed by process group at the deadline or when done | coordinator |
| cleanup | process groups killed, cluster stopped, directory removed on every path (RAII) | coordinator; `ReplayRoot` tests |
| no source rewrite | static commands never write; the harness imports the application unchanged; instrumentation lives in the worker client | — |
| no trace-to-code injection | reports carry SQL, frames and messages as data; the HTML report escapes all of them and loads/runs nothing (CSP `default-src 'none'`) | `the_html_report_escapes_runtime_observations` |
| no credential leakage | worker stderr is redacted (`safety::redact`) and truncated; the database user has no password | replay redaction tests |

Residual risks (unchanged from Phase 4): a malicious harness can consume
CPU and memory until the deadline, read files readable by the user
(macOS profile allows reads), and use local IPC not covered by the
profile. Do not run harnesses you would not run yourself. External
repositories are never executed automatically; no external application was
executed in Phase 7.
