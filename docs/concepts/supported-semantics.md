# Supported semantics

Midwrite models a deliberately narrow slice of TypeScript, Drizzle and
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
| interprocedural summaries | `check`, `trace` | same-file and imported functions, known-class methods, callbacks, IIFEs, inert helpers, bounded depth | fixtures, path-mode differentials (compact vs. legacy walk) |
| concurrency extraction to the model IR | `model-check`, `discover` | a Drizzle subset on PostgreSQL, source selectors, wrapped callbacks and closures, repositories, generated keys, replica routing, bounded loops, zod validation, request bodies ([source extraction](../architecture/source-extraction.md)) | extraction reports; PostgreSQL differentials; zod differential against the pinned runtimes; original-code runs |
| schema facts | `schema`, `model-check`, `discover` | Drizzle `pgTable`/`pgEnum`/`pgSchema` read statically, across modules ([schema extraction](../architecture/schema-extraction.md)) | catalog comparison on a disposable database |
| database model | `model-check` | PostgreSQL READ COMMITTED / REPEATABLE READ (validated subset), SERIALIZABLE (conservative abstraction), row locks, constraints, foreign keys, upserts, sequences, generated values, one async replica | lockstep differential against PostgreSQL 17.11 and 15.19; independently authored row-lock ground truth |
| fault model | `model-check` | crashes, retries, redelivery, lost acknowledgments, broken connections, pools, response loss ([failure model](failure-model.md)) | original-code runs on first-party fixtures, faults injected at verified checkpoints and through a protocol proxy ([fault injection](../architecture/fault-injection.md)) |
| runtime coordination | `--verify-source`, `replay` | Node.js ≥ 22, node-postgres 8.23.1 and postgres-js 3.4.5 with drizzle-orm 0.45.4; replay harnesses also Bun ≥ 1.2 | first-party fixtures only |

## Technologies

| | supported | not supported |
| --- | --- | --- |
| language | TypeScript (and TSX) source: `.ts`, `.tsx`, `.mts`, `.cts` | `.js` files are not discovered by `check`; no type checking; no other languages |
| ORM | Drizzle ORM (`drizzle-orm` pg-core; runtime pinned to 0.45.4) | other ORMs; SQLite executors (`.run()`, `.all()`, `.get()`); raw SQL is not parsed |
| database | PostgreSQL (model validated on 17.11 and 15.19) | other databases; PostgreSQL 16 and 14 not run in the differential matrix |
| drivers (runtime) | node-postgres `pg` 8.23.1; postgres-js 3.4.5 | Neon, Bun SQL and other drivers; other versions of postgres-js are refused |
| frameworks | Next.js route handlers and Server Actions (as entries), Fetch-style handlers with `request.json()` and `Response.json`; wrapped Server Actions through callback analysis or declared contracts | framework internals (the coordinator plays the client); `FormData` Server Actions; tRPC; `request.headers`, `formData()`, query strings |
| validation | zod 3 and zod 4 object schemas, version read per project ([zod](../reference/zod.md); differentially checked against zod 3.25.76 and 4.6.5) | refinements, transforms, preprocess, coerce, unions |
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
  so earlier scenarios keep their state spaces and reports (below).

## Versioned semantics

New semantics never change an earlier scenario silently. Every new IR or
scenario field is serde-defaulted and omitted when empty, so historical
scenarios, model reports and static outputs serialize byte-identically;
unknown feature names are refused.

| switch | effect | turned on by `discover` when |
| --- | --- | --- |
| scenario feature `foreign-keys` | enforces the source schema's foreign keys (and the row locks they take) | the table closure carries foreign keys |
| scenario feature `replicas` | routes reads through a modeled asynchronous replica | an actor reads through a replica |
| scenario feature `js-values` | JavaScript value semantics in extraction: arrays, strings, `undefined`, `Map`/`Set`, closures, loops, caught errors, zod, request bodies | always for extraction; kept in a scenario when an actor uses such a form |
| scenario features `timestamps`, `json` | time and JSON columns modeled | the tables have such columns |
| actor field `response: true` | a separate response step before every return | the actor's caller retries after a crash |
| `"returned": v` in an invariant | records return values (`Op::ReturnValue`) | a return-value property is declared |
