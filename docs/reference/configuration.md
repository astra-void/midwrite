# Configuration (`commitrift.json`)

Read by `check`, `trace`, `ir` and `init-replay` (`--config FILE`, default
`./commitrift.json`; the legacy `./crashproof.json` is read only when
`commitrift.json` does not exist, and the two are never merged). Every
field is optional; unknown fields are rejected. Discovery has its own file,
[`commitrift-contracts.json`](../user-guide/contracts.md); scenarios carry
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
    "covers": ["CP1001", "CP1002"],
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

A sample with several receivers: [`fixtures/commitrift.json`](../../fixtures/commitrift.json).

## Recovery declarations

Declarations are **assertions you make**; CommitRift never infers or
verifies them. Applied declarations appear in diagnostics with
`"source": "configuration"` and `"verified": false`.

| declaration | effect under `recommended` |
| --- | --- |
| `externalEffects[].bestEffort: true` (or `"recovery": "bestEffort"`) | CP1001/CP1005 whose missing effect is best-effort become `info` with a note; CP1002/CP1003 unchanged |
| `"recovery": "reconciled"` | a process outside the analyzed code re-derives this effect from committed state; CP1001/CP1005 about it become `info`; the window is still reported |
| `"recovery": "idempotentConsumer"` | receivers deduplicate repeats; a note only |
| `recoveryContracts[]` with `strategy: "retried"` | an outside mechanism re-executes the whole function after a failure; covered codes (any of CP1001–CP1005) become `info` with the cited evidence |
| `recoveryContracts[]` with `strategy: "reconciled"` | a separate process derives missing effects from committed state; can cover CP1001, CP1004, CP1005, not CP1002/CP1003 |

Covered findings keep their fingerprint. Details:
[Phase 4 recovery semantics](../phase4/RECOVERY-SEMANTICS.md).

## Trust model

* A bare name (`"db"`) matches any binding of that name unless the same
  file shows a contrary initializer (`const db = new Map()`). Prefer
  `importedFrom` in larger codebases.
* Member paths (`this.db`, `ctx.db`) are the weakest match: the root is
  resolved like a bare name, `this` is not resolved, properties are trusted
  as written; such findings are capped at `medium` confidence. An
  unconfigured lookalike (`this.repo.db`) is reported as `CP9001` with the
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
| `commitrift-contracts.json` | [contracts guide](../user-guide/contracts.md) |
| `commitrift-source.json`, `commitrift-replay.json` | authorized harness manifests: [source verification](../user-guide/source-verification.md) |
| environment | [CLI reference § environment variables](cli.md#environment-variables) |
