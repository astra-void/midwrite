# Release readiness (experimental public beta)

Phase 4 milestone 9. Lower priority than replay and correctness; this page
says what exists, what was verified, and what blocks a beta.

## Repository state

* Git: `main` with **no commits**, no remote. No commit was created, nothing
  was pushed, no repository or organization was created, no package was
  published — none of that was authorized. No repository URL, package link,
  download link or sponsor account is cited anywhere, because none exists.
* `.gitignore` (extended in Phase 4) excludes build output (`/target`),
  downloaded corpora (`/.corpora/`), demo output, local tool state (`.trail/`,
  `.claude/settings*.json`, `/graft/`), JavaScript dependencies
  (`node_modules/`), fixture build output (`/fixtures/replay/**/dist/`),
  generator test scratch directories, the supplied `digest.txt` and
  `.DS_Store`. Disposable PostgreSQL clusters and replay temporary
  directories live under the system temporary directory and are removed
  after each run. Lockfiles (`Cargo.lock`, `fixtures/replay/drizzle-node/package-lock.json`)
  and deliberate evidence under `reports/` are tracked. Reports from private
  corpus projects keep only aggregated numbers (trace dumps were deleted).

## Packaging

`scripts/package-release.sh [target]` builds `commitrift` with `--locked` and
writes `target/package/commitrift-<version>-<target>.tar.gz` (binary,
README, both licenses, `runtime/node`, the JSON Schemas) and a SHA-256 file.
GNU tar gets sorted entries, fixed owners and the commit time as mtime; BSD
tar (macOS) only fixed owners, so macOS archives are not bit-reproducible
yet.

| Target | Archive built | Static analysis verified | Isolated replay verified |
| --- | --- | --- | --- |
| `aarch64-apple-darwin` | yes (local) | yes (full test suite) | yes (`sandbox-exec`) |
| `aarch64-unknown-linux-gnu` | yes (Debian 12 container) | yes (full test suite; the packaged binary analyzes the fixtures) | yes (bubblewrap) |
| `x86_64-unknown-linux-gnu` | no | no | no |
| `x86_64-apple-darwin` | no | no | no |

Only the two verified targets may be called supported. The release workflow
(`.github/workflows/release.yml`) builds all four on a tag and attaches them
to a **draft** GitHub release for manual review; it has never run.

Not packaged: crates.io (the CLI `include_str!`s files from `runtime/node`
outside its crate directory, which `cargo package` would not include — the
files would have to move into the crate first) and npm (`@commitrift/replay`
is `private`; the scope's ownership is unverified).

## Installation methods today

1. From source: `cargo install --path crates/commitrift-cli` or
   `cargo build --release` (Rust ≥ 1.97).
2. A locally built archive (`scripts/package-release.sh`).

## CI

`.github/workflows/ci.yml`: fmt + clippy; static tests on Linux and macOS;
**required** replay on Linux (`COMMITRIFT_REQUIRE_REPLAY=1`, bubblewrap,
PostgreSQL, Node 22, Bun); SARIF and JSON Schema validation. Its replay job
was reproduced locally in a Debian container (245/245); it has not run on
GitHub. See [`LINUX-ISOLATION.md`](LINUX-ISOLATION.md#ci-githubworkflowsciyml).

## Documentation

README (product overview, scoped claims, installation, static example, the
Node + Drizzle replay workflow, supported platforms, security warnings, JSON
and SARIF, CI usage, limitations, contributing, license, funding status),
`docs/LIMITATIONS.md`, `docs/ARCHITECTURE.md`, `CONTRIBUTING.md`, and this
directory.

## Demonstration

`scripts/demo-phase4.sh` (log: `reports/phase4/demo.log`, 2026-10-25):

| Step | | Automated? |
| --- | --- | --- |
| 1 | Scan real TypeScript + Drizzle code | yes |
| 2 | Identify CP1001 (`4f53a6215ac16913`) | yes |
| 3 | Inspect the failure trace (text, JSON, HTML) | yes |
| 4 | Generate a replay harness; replay refuses the unfinished skeleton | yes |
| 5 | Configure observations, the invariant, schema and scenario call | **manual** (the demo shows the developer's completed version as a diff) |
| 6 | Authorized, isolated replay (`--repeat 5`: 5/5 reproduced) | yes |
| 7 | Observe the surviving PostgreSQL state (`ordersState="paid" queuePublishEvents=0`) | yes |
| 8 | Render the HTML verification report | yes |
| 9 | Compare with the transactional outbox (intent kept, relay retry delivers) | yes |

No mock data: every runtime value comes from a real PostgreSQL cluster and
a real Node.js process running the real Drizzle ORM.

## Release blockers

1. A real Git history and a verified public repository (owner decision).
2. A green run of `ci.yml` on GitHub-hosted runners, in particular the
   required Linux replay job on Ubuntu 24.04 (AppArmor user-namespace
   restriction, PostgreSQL 16 paths).
3. A tagged run of `release.yml` and a decision on supported targets
   (x86_64 Linux and macOS are unverified).
4. An independent security review of `commitrift-replay`, the sandbox-exec
   profile and the bubblewrap setup; resource limits for harnesses.
5. At least a small human-reviewed precision baseline (the review workflow
   exists; 0 reviews so far).
6. Packaging decisions: crates.io layout for the vendored runtime files, npm
   scope ownership, and how long to keep the legacy `crashproof` executable.
7. Reproducible macOS archives (GNU tar or an equivalent).
