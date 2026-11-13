# Installation

Midwrite is a Rust workspace. The CLI is on npm as `midwrite`, with
prebuilt binaries for macOS (arm64, x64), Linux (arm64, x64, static
musl) and Windows (x64, arm64; static analysis only); `npm install --save-dev
midwrite`. On other platforms, build it from
source. The replay harness client is published on npm as
`@midwrite/replay`; generated harnesses vendor their own copy.

## Requirements

| What | Needed for | Version used for the recorded evidence |
| --- | --- | --- |
| Rust toolchain | building | `rust-version = "1.97"` in `Cargo.toml`; built with rustc 1.98.1 (macOS) and 1.99.1 (Linux container) |
| PostgreSQL server binaries (`initdb`, `pg_ctl`, `postgres`) | `model-check --validate-postgres`, `--verify-source`, `schema --migrations`, `replay` | 17.11 (macOS, Homebrew `postgresql@17`), 15.19 (Linux container). Crash plans need PostgreSQL 14 or newer (`client_connection_check_interval`, [crashes](../architecture/runtime-verification.md#crashes)) |
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

`crates/midwrite-cli` has one default feature, `replay`, which pulls in
`midwrite-replay` and the model checker's PostgreSQL validation. Its
`Cargo.toml` comment says that without it the binary contains only static
analysis and report rendering. That configuration is not exercised by the
phase reports.

`scripts/package-release.sh` builds a local release archive (binary,
licenses, the replay client in `runtime/node`, JSON Schemas) with a SHA-256
checksum.

## Platforms

| Platform | npm / release binary | What has run on it |
| --- | --- | --- |
| macOS arm64 | yes | development machine (full suite, required replay); CI static tests on every push (`macos-15`); full suite in the v0.1.0 release run |
| macOS x64 | yes | full suite in the v0.1.0 release run (`macos-15-intel`) |
| Linux x64 | yes, static musl | CI static tests and required replay with bubblewrap on every push (`ubuntu-24.04`); full suite in the v0.1.0 release run |
| Linux arm64 | yes, static musl | Debian 12 container during development; full suite in the v0.1.0 release run (`ubuntu-24.04-arm`) |
| Windows x64, arm64 | yes, static analysis only (no `replay`) | smoke test only (`--version`, `check` on a fixture) in the v0.1.0 release run; the full suite needs PostgreSQL, bubblewrap and Unix sockets |

The release run tests on the runner's host target; the shipped Linux
binaries are cross-built for musl and smoke-tested. Which of these checks
passed, with run links and dates, and whether the published npm packages
install cleanly on each platform: [release status](../development/release-status.md).

Where OS isolation is unavailable, commands that execute code are blocked
(exit status 3) instead of running unisolated. Details:
[security reference](../reference/security.md).

## Next

* [Quickstart](quickstart.md): the static analyzer on one file.
* [First verification](first-verification.md): model checking, discovery
  and the optional runtime steps.
