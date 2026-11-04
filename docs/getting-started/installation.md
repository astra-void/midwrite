# Installation

Midwrite is a Rust workspace. There are no published binaries or
packages; build it from source.

## Requirements

| What | Needed for | Version measured in the phase reports |
| --- | --- | --- |
| Rust toolchain | building | `rust-version = "1.97"` in `Cargo.toml`; phases 7–13 built with rustc 1.98.1 (macOS) and 1.99.1 (Linux container) |
| PostgreSQL server binaries (`initdb`, `pg_ctl`, `postgres`) | `model-check --validate-postgres`, `--verify-source`, `schema --migrations`, `replay` | 17.11 (macOS, Homebrew `postgresql@17`), 15.19 (Linux container). Crash plans need PostgreSQL 14 or newer (`client_connection_check_interval`, [phase 8](../phase8/DIFFERENTIAL-VERIFICATION.md#postgresql-versions)) |
| Node.js ≥ 22 | original-code verification and replay harnesses | 22.23.2 (macOS), 22.23.4 (Linux container) |
| Bun ≥ 1.2 | optional replay-harness runtime (`init-replay --runtime bun`) | — |
| macOS `sandbox-exec` or Linux bubblewrap with unprivileged user namespaces | anything that executes code (`--allow-execution`) | bubblewrap 0.8.0 in the Linux container |

`midwrite check`, `ir`, `trace`, `explain`, `report`, `discover` and
`model-check` without validation flags need none of the optional
prerequisites: they only read files.

## Build

```bash
cargo install --path crates/midwrite-cli
```

or, without installing (the binary is `target/release/midwrite`):

```bash
cargo build --release
```

```text
$ midwrite --version
midwrite 0.1.0
```

The build also produces a legacy `crashproof` executable (the project was
called CrashProof until Phase 3; see the [README](../../README.md)).

`crates/midwrite-cli` has one default feature, `replay`, which pulls in
`midwrite-replay` and the model checker's PostgreSQL validation. Its
`Cargo.toml` comment says that without it the binary contains only static
analysis and report rendering. That configuration is not exercised by the
phase reports.

`scripts/package-release.sh` builds a local release archive (binary,
licenses, the replay client in `runtime/node`, JSON Schemas) with a SHA-256
checksum.

## Platforms

| Platform | Status |
| --- | --- |
| macOS arm64 | development machine of every phase; full test suite with required replay |
| Linux aarch64 (Debian 12 container on the same machine) | full test suite with required replay in phases 4–10 and 12 (Phase 7: 411 of the 412 macOS tests, see its [final report](../phase7/FINAL-REPORT.md#2-correctness)); Phase 11 was not run on Linux; Phase 13 Linux results are not in `reports/phase13/` at the time of writing |
| Linux x86_64, macOS x86_64 | not run |
| Windows | not supported; not run |

Where OS isolation is unavailable, commands that execute code are blocked
(exit status 3) instead of running unisolated. Details:
[security reference](../reference/security.md).

## Next

* [Quickstart](quickstart.md): the static analyzer on one file.
* [First verification](first-verification.md): model checking, discovery
  and the optional runtime steps.
