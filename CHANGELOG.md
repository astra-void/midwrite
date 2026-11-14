# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before 1.0.0, any minor release may change diagnostics, configuration or
output formats.

## [Unreleased]

### Added

- Promise continuations that are not analyzed in sequence are reported as
  coverage gaps (`MW9001`, reason `unmodeledContinuation`): an inline
  `.then`, `.catch` or `.finally` handler that contains a recognized
  operation, on a chain that is not awaited or passes several handlers.
  The notice gives the location, the reason and a next step, and lists
  every such continuation of the function. No ordering is invented
  (DEV-169).
- Reports say when the analysis is incomplete: `summary.knownGaps` and
  `summary.analysis.unmodeledContinuations` in JSON, a summary line in
  text, and run `properties` plus a `note` notification in SARIF. Zero
  known gaps is not a claim of complete recall (DEV-169).
- A pair of operations inside a helper whose parameter name is not a
  configured receiver (`async function save(store) { … }`) is reported
  where the caller binds the parameter to a recognized handle
  (`save(db)`). Before, neither the helper nor the caller reported it
  (DEV-181).
- Database handles held in fields that the constructor sets
  (`constructor(private readonly db: Database)`, or a sealed field
  assigned once from a constructor parameter) are followed.
  `new Repo(tx).save()` writes in the caller's transaction, and
  `new Repo(db).save()` commits on its own. This works through `this.m()`
  and imported classes. A pair inside one such method is reported where
  the method is called. Reassigned, public, accessor and inherited cases
  are refused (DEV-170).
- A two-pass blind AI re-review of the DEV-162 tier-1 sample (109 labels
  and the four upstream reports). It replaces the planned human review at
  the project owner's decision. 23 labels changed where two independent AI
  reviewers agreed against them, recorded as `ai-adjudication` with
  `humanReviewed: false`. The held-out hazard precision is now 28/30
  ([human review](docs/development/human-review.md#blind-ai-review-2026-11-13)).
- `.then`, `.catch` and `.finally` callbacks of an awaited promise run in
  the caller's sequence (`p` then `cb`, `try { p } catch { cb }`, `try {
  p } finally { cb }`), with the callback lowered in place
  ([evaluation harness](docs/development/evaluation-harness.md#promise-continuations-2026-11-13-dev-176)).
- A known SDK client held in a class field (`private client = new
  Resend(...)` set in the constructor) is recognized, and private methods
  (`this.#send()`) are followed. midday's audited window KP02 (an invoice
  email before the invoice update) and four other true positives are found
  ([evaluation harness](docs/development/evaluation-harness.md#sdk-clients-in-class-fields-and-private-methods-2026-11-13-dev-178)).
- Methods called on a field are followed into the class the field always
  holds: a `private`, `readonly` or `#private` field initialized with
  `new X()` or assigned once with it in the constructor. `classContracts`
  in `midwrite.json` declare the rest: the class a field holds, and the
  functions whose awaited result is an instance of a class (a `Proxy` or
  RPC stub). With a contract for zero's database Durable Object, its
  audited windows KP02–KP05 are found
  ([configuration](docs/reference/configuration.md#class-contracts)).
- A write whose `RETURNING` row is bound (`const [row] = await
  db.update(t)...returning()`, also `delete` and `insert ...
  onConflictDoNothing()`) no longer counts as a committed change on paths
  where the row is missing, including through a helper that returns the
  row. This does not apply to upserts, loops or callbacks. Three midday
  false positives give way to other pairs at the same lines
  ([evaluation harness](docs/development/evaluation-harness.md#writes-whose-returning-row-is-missing-2026-11-13-dev-180)).
- Properties of a returned object now decide a caller's branch through
  spreads (`{ ...r, consumed: true }`), `return f(...)`, effect-free helpers
  whose every return sets the property, type assertions, and truthiness or
  nullish tests of the binding. A test that comes before the object escapes
  to other code also counts. No evaluation finding changes
  ([evaluation harness](docs/development/evaluation-harness.md#returned-object-properties-across-helpers-2026-11-13-dev-175)).
- A release-status page that records, with run links and dates, what is
  verified about v0.1.0: the tagged release run, the GitHub release, the
  hand-published npm packages, clean installs on all six platforms and
  CI on `main`. It also records that CI on `main` is not reliably green,
  because one replay test fails intermittently. A dispatchable
  `install-check` workflow repeats the install and checksum checks
  ([release status](docs/development/release-status.md)).
- Tooling for an independent human review of the AI-written evaluation
  labels and upstream reports: a fixed 172-item sample, a blind reading
  packet, and checks for the review record. No item has been reviewed yet
  ([human review](docs/development/human-review.md)).
- lmsqueezy/nextjs-billing as a held-out evaluation project, pinned at
  `5ddd7680`, so the held-out precision gate runs again. Its first
  scoring is hazard precision 7/7, bug precision 1/7 and audited recall
  2/3. That is too few findings to replace the earlier pre-tuning estimate
  (122/127). All labels are AI-written.
- Three more held-out projects, chosen by a fixed sampling rule (the first
  eligible Drizzle + PostgreSQL applications in a star-ordered GitHub
  search): crafter-station/petdex, mickasmt/next-saas-stripe-starter and
  kriasoft/react-starter-kit, pinned and frozen on 2026-11-12. The
  four-project held-out set scores hazard precision 30/32 (94%, 95%
  interval 80–98%), bug precision 3/32 and audited recall 7/18. The gate
  passes. The README shows this scoring next to the historical pre-tuning
  one and keeps the two apart.

### Changed

- A call that leaves an optional argument out, or passes an object literal
  with a literal property, now decides the callee's branches on reads of
  that parameter (`const send = options?.send ?? false`, `if
  (options.purge)`), not only on destructured ones. When the parameter is
  passed to other code before the read, only a left-out or `undefined`
  argument decides it. No evaluation finding changes.

### Fixed

- Mutually exclusive growth and removal paths of an unchanged array are
  no longer combined into independent-commit hazards. Complementary pure
  filters provide a cardinality upper bound, and a nonempty `slice(count)`
  excludes `count > original.length`. Mutation, escaping aliases and
  unsupported predicates invalidate the proof. First-party regressions
  retain feasible write pairs, including sparse arrays and coercion cases.
- Awaited calls through local factory results (`const f = make(); await
  f()`, `await make()()`) now follow a single known returned function,
  including imported and inert forwarding factories. Plain factories must
  be synchronous and proven inert; async factories must be awaited first.
  Immutable captured parameters, locals, callbacks and recognized handles
  are retained per creation context. Mutable bindings and ambiguous returns
  remain unknown.
- Stable module-level factory bindings, aliases and named/default exports
  now resolve through inert initializer chains, including imports and
  barrels. Resolved exported callables are analyzed as entries without
  executing module code. Cycles, temporal dead zones, mutable bindings and
  effectful initializers remain unknown.
- A write reached through nested higher-order helpers (`withSpan("x", () =>
  withRetry(() => db.insert(t)))`, or a helper nested in itself with
  another callback) is no longer lost: such nesting is not treated as
  recursion, and running a callback does not use a level of
  `--max-call-depth`. Four new dev findings, all real windows.
- A module-private helper (not exported, only ever called directly) whose
  every call passes a transaction handle is no longer reported as
  independent commits, without needing `closedWorld`. This removes one dev
  false positive (midday `createSystemCategoriesForTeam`); held-out results
  are unchanged.

### Security

- `replay` harness processes now get the same per-process limits (CPU
  time, file size, open files, no core dumps) and process cleanup as
  source runs. The process group is killed after every exit, and detached
  processes are swept on Linux. Before this, only source runs had them,
  although the security reference listed them for both.
- Harnesses and project SQL connect to the disposable cluster as an
  ordinary role (`midwrite_app`). The superuser needs a per-cluster
  password. A harness could otherwise make the unsandboxed server run a
  program or write a file (`COPY … PROGRAM`).
- SQL files and observation queries with `psql` meta-commands (`\!`) or
  `COPY … PROGRAM` are refused, including `schema --migrations`.
- Ctrl-C, SIGTERM and SIGHUP stop the run and clean up the cluster, its
  shared memory and the run directory; a second signal exits at once.
- Linux isolation no longer mounts the whole home directory when the
  runtime lives in `~/bin` or the home directory is a Git work tree.
- Programs are looked up only in absolute `PATH` entries, `/usr/bin/bwrap`
  is preferred, `git` runs without `core.fsmonitor`, and a source-harness
  manifest must be named `midwrite-source*.json`.
- Known limitation: on macOS 27 detached harness processes cannot be
  found after a run ([security](docs/reference/security.md#review-of-2026-11-12-dev-163)).

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
  `-linux-arm64`, `-linux-x64`, `-win32-x64`, `-win32-arm64`; Linux builds are static musl,
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
