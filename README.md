# Midwrite

> **v0.1.0: experimental.** Midwrite is a young static analyzer with a
> narrow scope. It misses real problems, and some of what it reports is
> intentional in your code. Read this page before relying on it.

Midwrite looks at TypeScript backends for places where a crash, restart,
timeout or failed call **between two separately committed side effects**
leaves the system half-done. For example, the database says an order is
paid, but the `order.completed` event was never published.

It reads your source and never runs it. No account, no network, no
telemetry.

## A real bug it found

[Kan](https://github.com/kanbn/kan)'s Stripe checkout webhook creates a
workspace, then its subscription, as two separate writes:

```ts
// Reduced from Kan's checkout.session.completed handler.
if (!existingWorkspace) {
  await workspaceRepo.create(db, workspaceInput);   // commits
  // A failure here leaves a workspace without its subscription.
  await subscriptionRepo.create(db, subscriptionInput);
}
// When Stripe retries, existingWorkspace is present, so neither create runs.
```

Midwrite found this across modules, through the repository helpers
(`MW1004`). If the second write fails, Stripe redelivers the event, the
guard sees the workspace, and the subscription is never created.

What this example does and doesn't show:

* The failure path was checked by hand against the source and
  [reported upstream (kanbn/kan#697)](https://github.com/kanbn/kan/issues/697).
* It has **not** been reproduced at runtime, and the maintainers have not
  responded yet.
* It is one of four reports from a
  [review of five open-source backends](docs/development/upstream-review.md).
  The other three are
  [rybbit-io/rybbit#1355](https://github.com/rybbit-io/rybbit/issues/1355),
  [vercel/chatbot#1567](https://github.com/vercel/chatbot/issues/1567) and
  [openstatusHQ/openstatus#3077](https://github.com/openstatusHQ/openstatus/issues/3077).
  All four are still open and unconfirmed.

## What a warning looks like

```ts
export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
  // <- a crash here leaves a paid order without an event
  await queue.publish("order.completed", { orderId });
}
```

```text
warning[MW1001]: database commit followed by an independent external effect
  --> src/orders.ts:4:9 (in `completeOrder`, confidence: high)
    |
  4 |   await queue.publish("order.completed", { orderId });
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  = `db.update(orders)` commits before `queue.publish("order.completed", { orderId })` (message-publish); a failure in between leaves the database changed without the external effect
  failure sequence:
    1. `db.update(orders)` (2:9) commits immediately: it is not inside a recognized transaction.
    2. The process crashes or is restarted, or the call fails, after the commit and before `queue.publish("order.completed", { orderId })` completes.
    3. `queue.publish("order.completed", { orderId })` is never performed.
    4. Result: the database change is durable but the external effect never happens, and nothing records that it is still owed.
  help: Record the intent in the same database transaction as the state change (transactional outbox) and perform the external effect from a relay that retries until it succeeds, or make a reconciliation job derive the missing effect from database state.
```

| Code | Default severity | Meaning |
| --- | --- | --- |
| `MW1001` | warning | A database commit, then an external effect |
| `MW1002` | warning | An external effect, then a database commit |
| `MW1003` | warning | An external effect inside a transaction, before it commits |
| `MW1004` | info (warning when a write bypasses its transaction) | Two independent database commits |
| `MW1005` | info | Two independent external effects |
| `MW9001` | info | Not a finding: Midwrite could not analyze something here |

`midwrite explain MW1001` prints the full explanation of a code.

## What a warning means

When Midwrite reports a window, the window almost always exists. Whether it
is a bug is a separate question, and the answer is "no" more often than
"yes".

The best estimate of how it behaves on code it has never seen is the last
scoring of three projects (Kan, Dittofeed, vercel/chatbot) that were still
held out on 2026-11-12. The analyzer had not been tuned against them at
that point, and Midwrite reported 127 hazards:

| | count | share |
| --- | ---: | ---: |
| The reported window exists in the code | 122 / 127 | 96% |
| …and nothing in the app handles it (a likely bug) | 57 / 127 | 45% |
| …but the app accepts or repairs it (best-effort email, job retries, webhook redelivery) | 65 / 127 | 51% |
| False positive: the window is not real | 5 / 127 | 4% |

So treat a warning as **a place worth five minutes of review**, not as a
confirmed bug. Midwrite cannot see retries, reconciliation jobs or
idempotent consumers that live outside the analyzed code. You can declare
them (see [configuration](docs/reference/configuration.md#recovery-declarations)).

How much to trust these numbers:

* **There is no independent held-out set right now.** After that scoring,
  the three projects were moved to the dev set and the analyzer was tuned
  on their misses and false positives. On those same projects it now scores
  98% (165/168) of windows real and 51% (86/168) likely bugs. That is a
  tuned result and says little about your code. New held-out projects have
  to be audited and frozen before there is a fresh estimate.
* **Every label was written by AI agents, not by people.** The agents that
  labeled findings were separate from the ones that audited the code, but
  no human has reviewed the labels yet.
* The 0.9 precision gate passed only after an earlier set change. The first
  scoring was 0.89, and one project that scored 0.60 was moved to the dev
  set so its false positives could be fixed.
* On the dev projects, which the rules were tuned on before 2026-11-12, it
  was lower: 84%.

The details are in the [evaluation harness](docs/development/evaluation-harness.md).

## What it misses

Recall is the weak side. Expect Midwrite to stay silent about real
problems.

* In the last held-out scoring it found **49 of 61** known windows (80%).
  That denominator comes from capped AI audits of code that already uses
  the supported stack, so the real miss rate on your code is likely higher.
* Where effects went through helpers it couldn't follow, recall dropped. On
  Dittofeed (ClickHouse and Temporal behind helpers) it found **11 of 20**.
  After tuning on these projects it finds all 71 known windows there. That
  shows the specific gaps were fixed, not that recall is now complete on
  other code.
* Anything outside the supported stack below is **not seen at all**.

Common reasons it stays silent:

* Methods on values whose class it can't determine: parameters, SDK clients,
  factory results.
* Packages in `node_modules`, dynamic `import()`, CommonJS `require`.
* Calls more than 3 levels deep (`--max-call-depth`), recursion,
  `.then()` continuation bodies, callbacks run by code it doesn't analyze.
* Writes through anything other than Drizzle: raw SQL, Prisma, Supabase
  table writes, other ORMs.
* Second and later loop iterations, and top-level module code.

When it notices something it can't follow next to a recognized effect, it
says so with `MW9001` instead of guessing. The full list is in
[limitations](docs/reference/limitations.md).

## Supported stack

| | Supported today |
| --- | --- |
| Language | TypeScript (`.ts`, `.tsx`, `.mts`, `.cts`), parsed with [Oxc](https://oxc.rs) |
| Database | **Drizzle ORM on PostgreSQL** only: writes, reads, `db.transaction(...)`, savepoints |
| External effects | What you list in `midwrite.json`, plus built-in rules for Stripe, Resend, BullMQ, Redis (ioredis, redis, Upstash), Supabase storage and auth admin, S3 (including multipart uploads), ClickHouse inserts, Polar, Nodemailer, SendGrid, Twilio, Vercel Blob |
| Code structure | Functions in the same file and in imported files (relative imports, `tsconfig` `paths`, workspace packages), classes with known receivers, object services, callbacks passed to analyzed functions |
| Platforms | Built and tested on macOS arm64 and Linux aarch64. Linux and macOS x86_64 are expected to work but have not been run. Windows is not supported. |

**Not supported:** Prisma, TypeORM, Kysely, Sequelize, MikroORM, raw SQL,
MySQL, SQLite and libSQL dialect semantics, and plain `.js`/`.jsx` files
(they are not scanned). More detail is in
[supported patterns](docs/reference/supported-patterns.md).

## Install

The `midwrite` CLI is not published to a package registry. Build it from
source with Rust 1.97 or newer:

```bash
cargo install --path crates/midwrite-cli
```

If a GitHub release for `v0.1.0` exists, it carries prebuilt archives for
the platforms its release run passed on, with SHA-256 checksums.

The JavaScript client for replay harnesses is on npm as
[`@midwrite/replay`](https://www.npmjs.com/package/@midwrite/replay). You
only need it if you write a harness by hand; `midwrite init-replay`
vendors its own copy.

## Use

```bash
midwrite check .
```

```bash
midwrite check ./src --format json
```

`midwrite.json` tells it which names are your database handle and which
calls are side effects:

```json
{
  "databaseReceivers": [{ "name": "db", "importedFrom": "@/server/db" }],
  "externalEffects": [
    { "receiver": "queue", "method": "publish", "category": "message-publish" },
    { "function": "sendEmail", "category": "email", "importedFrom": "@/lib/email" }
  ]
}
```

Configuration entries are trusted as written. Prefer `importedFrom`, so a
parameter that happens to be named `db` doesn't match. See
[configuration](docs/reference/configuration.md).

To adopt it in CI without fixing every existing finding first, record a
baseline:

```bash
midwrite check . --write-baseline midwrite-baseline.json
```

```bash
midwrite check . --baseline midwrite-baseline.json
```

Exit status: `0` clean, `1` a diagnostic at or above `--fail-on` (default
`warning`), `2` usage or configuration error, `3` some files could not be
parsed. The [quickstart](docs/getting-started/quickstart.md) and the
[`check` guide](docs/user-guide/check.md) cover policies, inline
suppressions and output formats (text, JSON, SARIF).

## More experimental commands

`check` is the part this release is about. The binary also contains
heavier tools that are less tested on real code. Their evidence comes only
from first-party fixtures:

* `midwrite trace`: the failure path behind a finding, as a static
  description.
* `midwrite replay --allow-execution`: runs a project-owned harness against
  a disposable PostgreSQL, kills it at a checkpoint, and checks the
  surviving state. **This executes code.**
* `midwrite model-check` and `midwrite discover`: bounded model checking of
  concurrent and crash/retry scenarios against a model of PostgreSQL.

No bug in an external application has been confirmed with these tools. See
[`docs/README.md`](docs/README.md) for guides, and
[security](docs/reference/security.md) before running anything with
`--allow-execution`.

## Documentation

* [Docs index](docs/README.md)
* [Limitations](docs/reference/limitations.md)
* [CLI reference](docs/reference/cli.md)
* [Diagnostics](docs/DIAGNOSTICS.md)
* [Architecture](docs/ARCHITECTURE.md)
* [Evidence index](docs/research/evidence-index.md)
* [Changelog](CHANGELOG.md)

## Feedback

This release exists to learn where Midwrite is wrong. A false positive, a
missed window, or a finding you couldn't understand are all useful reports.
Templates are in [`docs/alpha`](docs/alpha/README.md).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and
[testing](docs/development/testing.md). New behavior comes with a fixture
under `fixtures/` whose `// @expect` annotations state the exact diagnostics.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
project shall be dual licensed as above, without any additional terms or
conditions.
