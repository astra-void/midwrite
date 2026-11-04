# Supported semantics

CommitRift models a deliberately narrow slice of TypeScript, Drizzle and
PostgreSQL. This page states the slice per layer; pattern-level tables are
in the [supported-patterns reference](../reference/supported-patterns.md)
and everything outside the slice is in the
[limitations reference](../reference/limitations.md). **Not every
PostgreSQL feature is modeled**, and anything not modeled is reported as
unknown or unsupported rather than approximated.

## Layers

| layer | used by | technology | how it is checked |
| --- | --- | --- | --- |
| TypeScript recognition and lowering to the Effect IR | `check`, `trace`, `ir` | `.ts/.tsx/.mts/.cts`, Oxc parser and semantic analysis; no type checker | `@expect` fixtures, corpus byte-identity |
| interprocedural summaries | `check`, `trace` | same-file and imported functions, known-class methods, callbacks, IIFEs, inert helpers, bounded depth | fixtures, path differentials (Phase 7–8) |
| concurrency extraction to the model IR | `model-check`, `discover` | a Drizzle subset on PostgreSQL, source selectors, wrapped callbacks, repositories, generated keys, replica routing | extraction reports; PostgreSQL differentials; original-code runs |
| schema facts | `schema`, `model-check`, `discover` | Drizzle `pgTable`/`pgEnum`/`pgSchema` read statically, across modules | catalog comparison on a disposable database (Phase 8) |
| database model | `model-check` | PostgreSQL READ COMMITTED / REPEATABLE READ (validated subset), SERIALIZABLE (conservative abstraction), row locks, constraints, foreign keys, upserts, sequences, generated values, one async replica | lockstep differential against PostgreSQL 17.11 and 15.19; independent lock ground truth (Phase 13) |
| fault model | `model-check` | crashes, retries, redelivery, lost acknowledgments, broken connections, pools, response loss | original-code runs on first-party fixtures |
| runtime coordination | `--verify-source`, `replay` | Node.js ≥ 22, node-postgres 8.23.1 and postgres-js 3.4.5 with drizzle-orm 0.45.4; replay harnesses also Bun ≥ 1.2 | first-party fixtures only |

## Technologies

| | supported | not supported |
| --- | --- | --- |
| language | TypeScript (and TSX) source: `.ts`, `.tsx`, `.mts`, `.cts` | `.js` files are not discovered by `check`; no type checking; no other languages |
| ORM | Drizzle ORM (`drizzle-orm` pg-core; runtime pinned to 0.45.4) | other ORMs; SQLite executors (`.run()`, `.all()`, `.get()`); raw SQL is not parsed |
| database | PostgreSQL (model validated on 17.11 and 15.19) | other databases; PostgreSQL 16 and 14 not run in the differential matrix |
| drivers (runtime) | node-postgres `pg` 8.23.1; postgres-js 3.4.5 | Neon, Bun SQL and other drivers; other versions of postgres-js are refused |
| frameworks | Next.js route handlers and Server Actions (as entries), with `request.json()` and `Response.json` (Phase 16); wrapped Server Actions through callback analysis or declared contracts | framework internals (the coordinator plays the client); `FormData` Server Actions; tRPC |
| validation | zod 3 and zod 4 object schemas (Phase 16 subset, differentially checked against zod 3.25.76 and 4.6.5) | refinements, transforms, preprocess, coerce, unions |
| external effects | configured or declared callees (`queue.publish`, `stripe.charges.create`, …) | unconfigured SDK calls are unknown operations |

## Principles that hold at every layer

* **Unknown is first-class.** A call, expression or SQL shape that is not
  understood becomes an `Unknown` effect or operation with its reason; it
  is never dropped and never upgraded to a known kind.
* **No guessing from names.** Receivers, retries, delivery, idempotency,
  invariants and replica factories come from configuration or contracts;
  each such input is printed as an assumption.
* **Refuse rather than approximate.** A scenario whose semantics the model
  does not represent (for example `ON DELETE CASCADE` reached by a
  statement) does not load.
* **Versioned semantics.** New model semantics are opt-in per scenario
  (`features`, `response`, `returned`) so earlier scenarios keep their
  state spaces and reports ([Phase 13 architecture § 3](../phase13/ARCHITECTURE.md#3-versioned-semantics-track-g2)).
