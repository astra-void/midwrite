# midwrite

> **v0.1.0: experimental.** A young static analyzer with a narrow scope.
> It misses real problems, and some of what it reports is intentional in
> your code. Read the [project README](https://github.com/astra-void/midwrite#readme)
> before relying on it.

Midwrite looks at TypeScript backends for places where a crash, restart,
timeout or failed call between two separately committed side effects
leaves the system half-done. For example, the database says an order is
paid, but the `order.completed` event was never published.

It reads your source and never runs it. No account, no network, no
telemetry.

```bash
npx midwrite check .
```

```bash
npm install --save-dev midwrite
```

## Scope

* **Supported:** TypeScript with **Drizzle ORM on PostgreSQL**, configured
  external effects, and built-in rules for common SDKs (Stripe, Resend,
  BullMQ, Redis, S3, ClickHouse and others).
* **Not supported:** Prisma and other ORMs, raw SQL, other databases,
  plain `.js` files. On Windows only static analysis is available (`check`,
  `trace`, `ir`, `explain`, `report`); runtime replay and model checking
  against PostgreSQL need macOS or Linux.

## What a warning means

In the last scoring on projects the analyzer had not been tuned against,
96% of reported windows existed in the code, but only 45% were bugs the
application did not already handle. The rest were accepted or repaired
by the application. It found 80% of known windows. All labels were
written by AI agents, not people. Treat a warning as a place worth
reviewing, not a confirmed bug.

Details: [what a warning means](https://github.com/astra-void/midwrite#what-a-warning-means),
[what it misses](https://github.com/astra-void/midwrite#what-it-misses),
[limitations](https://github.com/astra-void/midwrite/blob/main/docs/reference/limitations.md).

## How this package works

This package is a small launcher. npm installs one platform package with
the native binary for your system:

| platform | package |
| --- | --- |
| macOS arm64 | `@midwrite/cli-darwin-arm64` |
| macOS x64 | `@midwrite/cli-darwin-x64` |
| Linux arm64 (static, musl) | `@midwrite/cli-linux-arm64` |
| Linux x64 (static, musl) | `@midwrite/cli-linux-x64` |
| Windows x64 (static analysis only) | `@midwrite/cli-win32-x64` |

Installing with `--omit=optional` skips the binary, and the launcher
then exits with an error. On other platforms, build from source.

## License

MIT OR Apache-2.0, at your option.
