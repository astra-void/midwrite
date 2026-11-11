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
  (Stripe, Resend, BullMQ, Redis, Supabase storage and auth admin, S3,
  Polar, Nodemailer, SendGrid, Twilio, Vercel Blob), matched by provenance
  rather than by name.
- Opt-in `externalEffects[].httpMethods` to keep read-only `fetch` calls
  from matching an HTTP write rule.
- Cross-module call summaries: relative imports, `tsconfig` `paths`,
  workspace packages, classes with known receivers, object services and
  callbacks.
- Policies (`recommended`, `strict`, `audit`), baselines, inline
  suppressions with a required reason, recovery declarations, and text,
  JSON and SARIF output.
- Experimental, less-tested commands: `trace`, `replay` (executes a
  project-owned harness only with `--allow-execution`), `init-replay`,
  `report`, `model-check`, `discover` and `schema`.
- An evaluation harness with separate dev and held-out project sets
  (`scripts/eval.py`). On held-out projects: hazard precision 96%
  (122/127), bug precision 45% (57/127), recall 80% (49/61). All labels are
  AI-written and have not been reviewed by people.

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
- Prebuilt archives cover only the platforms on which the release workflow
  passed. Windows is not supported.

See [`docs/reference/limitations.md`](docs/reference/limitations.md).

[Unreleased]: https://github.com/astra-void/midwrite/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/astra-void/midwrite/releases/tag/v0.1.0
