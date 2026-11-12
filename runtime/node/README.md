# @midwrite/replay

> **Experimental (0.1.0).** This package is only useful together with the
> `midwrite` command-line tool of the same version. Its protocol can change
> in any minor release before 1.0.

The JavaScript side of Midwrite's opt-in runtime checks. A replay harness
uses it to report checkpoints around real database calls, so that
`midwrite replay` can kill or fail the process at a chosen point and then
inspect what survived. It has no dependencies and runs unchanged on
Node.js ≥ 22 and Bun ≥ 1.2.

You normally don't install this by hand. `midwrite init-replay` writes a
harness that vendors a copy of this client matching the binary's protocol
version (`file:./midwrite-runtime`). Install from npm only if you write a
harness yourself:

```bash
npm install @midwrite/replay
```

## Entry points

| import | what it provides |
| --- | --- |
| `@midwrite/replay` | `checkpoint(name, label)`, `runScenarios(...)`, `replayActive()`, `replayDatabase()`, `InjectedFault` |
| `@midwrite/replay/pg` | `instrumentPgPool(pool)`: wraps a node-postgres pool and emits checkpoints for the statements your ORM sends; `replayPoolConfig()` |
| `@midwrite/replay/event-log` | `publishToEventLog(topic, payload)`: an fsync'ed append, used as a stand-in external transport |
| `@midwrite/replay/source` | the worker used by `midwrite model-check --verify-source` to run original application code under a schedule |

Outside `midwrite replay`, every checkpoint is a no-op.

## Safety

Code that uses this package runs only when you call `midwrite replay` or
`midwrite model-check --verify-source` with `--allow-execution`. Midwrite
gives it a disposable PostgreSQL and OS isolation, which protects against
accidents in a trusted harness. It is **not** a sandbox for untrusted code.

## Documentation

The replay protocol, the harness format and the security model are
documented in the Midwrite repository under
`docs/architecture/replay-protocol.md` and `docs/reference/security.md`.

## License

MIT OR Apache-2.0, at your option.
