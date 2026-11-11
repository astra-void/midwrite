# Configuration (`midwrite.json`)

Read by `check`, `trace`, `ir` and `init-replay` (`--config FILE`, default
`./midwrite.json`; the legacy `./crashproof.json` is read only when
`midwrite.json` does not exist, and the two are never merged). Every
field is optional; unknown fields are rejected. Discovery has its own file,
[`midwrite-contracts.json`](../user-guide/contracts.md); scenarios carry
their own `extraction` section ([model-check](../user-guide/model-check.md#scenario-files)).

## Fields

```json
{
  "databaseReceivers": ["db", "this.db", "ctx.db", { "name": "getDb()", "importedFrom": "@/server/db" }],
  "transactionMethods": ["transaction"],
  "externalEffects": [
    { "receiver": "queue", "method": "publish", "category": "message-publish" },
    { "receiver": "stripe.charges", "method": "create", "category": "payment" },
    { "function": "sendEmail", "category": "email", "importedFrom": "@/lib/email" },
    { "function": "triggerJob", "firstArgument": "notification", "category": "message-publish", "bestEffort": true }
  ],
  "exclude": ["tests/**", "**/*.test.ts", "scripts/**"],
  "recoveryContracts": [{
    "function": "ProcessDocumentProcessor.process",
    "file": "apps/worker/src/processors/documents/process-document.ts",
    "strategy": "retried",
    "covers": ["MW1001", "MW1002"],
    "evidence": "queues/documents.config.ts:17 attempts 3; stalled jobs are re-run once",
    "limits": "gives up after 3 attempts"
  }]
}
```

| field | default | meaning |
| --- | --- | --- |
| `$schema` | — | ignored by the analyzer (editor support) |
| `databaseReceivers` | `["db"]` | names, member paths (`this.db`, `ctx.db`) or argument-free calls (`getDb()`) that denote the Drizzle database; `{ "name", "importedFrom" }` requires the binding to be imported from that exact specifier |
| `transactionMethods` | `["transaction"]` | methods that open a transaction on a database receiver |
| `externalEffects[]` | one rule: `queue.publish` (`message-publish`) | `receiver` + `method`, or `function`; `category` (`message-publish`, `http-request`, `email`, `payment`, `file-write`, `other`); optional `importedFrom`, `firstArgument` (string literal), `bestEffort`, `recovery`; rules are tried in order |
| `exclude` | `[]` | glob patterns skipped when a directory is scanned (with `/`: relative to the scanned directory; without: each file and directory name) |
| `recoveryContracts[]` | `[]` | function-level recovery mechanisms, below |

A sample with several receivers: [`fixtures/midwrite.json`](../../fixtures/midwrite.json).

## Narrowing HTTP effects

A broad `fetch` rule also matches read-only downloads and metadata lookups.
Use `httpMethods` to restrict a fetch-style rule to explicit methods:

```json
{ "function": "fetch", "category": "http-request", "httpMethods": ["POST", "PUT", "PATCH", "DELETE"] }
```

The field is optional and requires `category: "http-request"` and a nonempty
list of uppercase method names. It matches only a string-literal `method`
in the second argument's object literal. Request method strings are normalized
to uppercase. A later spread, computed key, getter, or dynamic method invalidates
the match; a later explicit literal `method` establishes it again.

Unmatched calls remain unknown, rather than being declared pure. They may
produce MW9001 around recognized effects. This includes implicit GETs,
`Request` objects without explicit options, and options passed through variables
or helpers. Configure known mutating helpers separately when their implementation
passes options dynamically. A later unrestricted rule still matches those calls.
Methods alone do not prove mutation: a POST used for a read-only query can still
be a false positive, and a GET endpoint that mutates state needs its own rule.
Existing rules without `httpMethods` retain their behavior.

## Recovery declarations

Declarations are **assertions you make**; Midwrite never infers or
verifies them. Applied declarations appear in diagnostics with
`"source": "configuration"` and `"verified": false`.

| declaration | effect under `recommended` |
| --- | --- |
| `externalEffects[].bestEffort: true` (or `"recovery": "bestEffort"`) | MW1001/MW1005 whose missing effect is best-effort become `info` with a note; MW1002/MW1003 unchanged |
| `"recovery": "reconciled"` | a process outside the analyzed code re-derives this effect from committed state; MW1001/MW1005 about it become `info`; the window is still reported |
| `"recovery": "idempotentConsumer"` | receivers deduplicate repeats; a note only |
| `recoveryContracts[]` with `strategy: "retried"` | an outside mechanism re-executes the whole function after a failure; covered codes (any of MW1001–MW1005) become `info` with the cited evidence |
| `recoveryContracts[]` with `strategy: "reconciled"` | a separate process derives missing effects from committed state; can cover MW1001, MW1004, MW1005, not MW1002/MW1003 |

Covered findings keep their fingerprint. Details:
[Phase 4 recovery semantics](../phase4/RECOVERY-SEMANTICS.md).

## Trust model

* A bare name (`"db"`) matches any binding of that name unless the same
  file shows a contrary initializer (`const db = new Map()`). Prefer
  `importedFrom` in larger codebases.
* Member paths (`this.db`, `ctx.db`) are the weakest match: the root is
  resolved like a bare name, `this` is not resolved, properties are trusted
  as written; such findings are capped at `medium` confidence. An
  unconfigured lookalike (`this.repo.db`) is reported as `MW9001` with the
  path to add.
* `importedFrom` compares the specifier verbatim (`"@/db"` and `"../db"`
  are different).
* Handles created by `drizzle(...)` from a `drizzle-orm*` module and
  transaction callback parameters are recognized by provenance, whatever
  the configuration says.

Source of truth for the analyzer:
[README § configuration](../../README.md#configuration),
[`docs/LIMITATIONS.md` § recognition trust model](../LIMITATIONS.md#recognition-trust-model).

## Configuration in other commands

| where | what |
| --- | --- |
| scenario `extraction` | `root`, `tables`, `externals` (`callee`, `channel`, `keyArgument`), `schema`, `transactionRetries`, `maxInlineDepth`, `replicaFactories` |
| `midwrite-contracts.json` | [contracts guide](../user-guide/contracts.md) |
| `midwrite-source.json`, `midwrite-replay.json` | authorized harness manifests: [source verification](../user-guide/source-verification.md) |
| environment | [CLI reference § environment variables](cli.md#environment-variables) |
