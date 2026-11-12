# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before 1.0.0, any minor release may change diagnostics, configuration or
output formats.

## [Unreleased]

## [0.1.0] - 2026-11-12

First public release, **experimental**. The README states what this
release can and cannot do, with the measured precision and recall.

### Added

- `midwrite check`: static failure-path analysis of TypeScript for windows
  between independently committed side effects (`MW1001`–`MW1005`), with a
  failure sequence, confidence and mitigation for each finding, and
  `MW9001` notes where analysis is incomplete.
- Drizzle ORM on PostgreSQL: writes, reads, transactions, savepoints and
  writes that bypass their enclosing transaction.
- Configured external effects, and built-in rules for common SDK clients
  (Stripe, Resend, BullMQ, Redis, Supabase storage and auth admin, S3
  including multipart `Upload`, ClickHouse `insert`, Polar, Nodemailer,
  SendGrid, Twilio, Vercel Blob), matched by provenance rather than by name.
- Opt-in `externalEffects[].httpMethods` to keep read-only `fetch` calls
  from matching an HTTP write rule.
- Configuration for zero-argument factory receivers (`"getAnalytics()"`),
  asserted `resolvedResult` values of configured operations, and an opt-in
  `closedWorld` assumption for helpers that are only called with a
  transaction.
- Cross-module call summaries: relative imports, `tsconfig` `paths`,
  workspace packages, classes with known receivers, object services and
  callbacks.
- Policies (`recommended`, `strict`, `audit`), baselines, inline
  suppressions with a required reason, recovery declarations, and text,
  JSON and SARIF output.
- The CLI on npm as `midwrite`, a launcher that installs one prebuilt
  binary package (`@midwrite/cli-darwin-arm64`, `-darwin-x64`,
  `-linux-arm64`, `-linux-x64`, `-win32-x64`; Linux builds are static musl,
  the Windows build has static analysis only).
- `@midwrite/replay` on npm: the dependency-free replay harness client
  (checkpoints, `pg` adapter, event log, source worker) for Node.js and
  Bun.
- Experimental, less-tested commands: `trace`, `replay` (executes a
  project-owned harness only with `--allow-execution`), `init-replay`,
  `report`, `model-check`, `discover` and `schema`.
- An evaluation harness with separate dev and held-out project sets
  (`scripts/eval.py`). The last held-out scoring, before any tuning on those
  projects, measured hazard precision 96% (122/127), bug precision 45%
  (57/127) and recall 80% (49/61). Those projects have since moved to dev
  and the analyzer was tuned on them, so there is currently no independent
  held-out set. All labels are AI-written and have not been reviewed by
  people.

### Known limitations

- Drizzle on PostgreSQL only. Prisma, other ORMs, raw SQL and other
  databases are not analyzed.
- Recall is limited: calls on values of unknown class, `node_modules`
  packages, deep or recursive call chains and later loop iterations are
  not followed.
- Most findings that describe a real window are not bugs, because the
  application accepts or repairs the partial state in ways Midwrite cannot
  see.
- No finding in an external application has been reproduced at runtime.
- The full test suite runs on macOS arm64 and Linux. The npm binaries are
  built natively in CI and only smoke-tested (`check` on the vulnerable
  fixtures) on their own OS. Windows has static analysis only: no runtime
  replay or PostgreSQL validation.

See [`docs/reference/limitations.md`](docs/reference/limitations.md).

[Unreleased]: https://github.com/astra-void/midwrite/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/astra-void/midwrite/releases/tag/v0.1.0
