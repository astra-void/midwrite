# Configuration (`midwrite.json`)

Read by `check`, `trace`, `ir` and `init-replay` (`--config FILE`, default
`./midwrite.json`). Every
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
| `builtinEffects` | `true` | recognize writes of well-known SDK clients without an `externalEffects` entry (below); `false` turns them off |
| `closedWorld` | `false` | project analysis may use observed transaction arguments to refine standalone helpers; enable only when all callers are analyzed and there are no external or dynamic callers |

## Built-in SDK effects

With `builtinEffects` (the default), calls that change state through these
SDKs are external effects without a configuration entry. Configured rules
are tried first.

| package | client | effects (category) |
| --- | --- | --- |
| `resend` | `new Resend()` | `emails.send`, `batch.send`, `broadcasts.*` (email); `contacts`/`audiences`/`domains` `create`/`update`/`remove` (other) |
| `stripe` | `new Stripe()` | `<resource>.create/update/del/cancel/confirm/capture/refund…` (payment); reads such as `retrieve`/`list` and `webhooks.*` are not effects |
| `@supabase/supabase-js`, `@supabase/ssr` | `createClient()`, `createServerClient()`, `SupabaseClient` | `storage.from(b).upload/update/move/copy/remove` (file-write); `auth.admin.createUser/deleteUser/updateUserById/inviteUserByEmail/generateLink/signOut` (other). Table writes (`from(t).insert(...)`) are not modeled |
| `bullmq` | `new Queue()`, `Job`, `FlowProducer`, `queue.getJob()` | queue `add/addBulk/remove/drain/clean/obliterate/…`, job `remove/retry/promote/update/moveTo…` (message-publish) |
| `ioredis`, `redis`, `@upstash/redis` | `new Redis()`, `createClient()`, `Redis.fromEnv()` | write commands (`set`, `del`, `incr`, `expire`, `hset`, `lpush`, `publish`, …), and `multi()/pipeline() … exec()` with a queued write (other) |
| `@polar-sh/sdk` | `new Polar()` | `<resource>.create/update/delete/revoke` (payment) |
| `nodemailer` | `createTransport()` | `sendMail` (email) |
| `@aws-sdk/client-s3` | `new S3Client()` | `send(new XCommand(...))` except `Get*`/`Head*`/`List*`/`Select*`, and v2-style `putObject`/`deleteObject`/… (file-write) |
| `@aws-sdk/lib-storage` | `new Upload()` | `done()` (file-write) |
| `@clickhouse/client`, `@clickhouse/client-web` | `createClient()`, `ClickHouseClient` | `insert()` (other); arbitrary SQL through `command`/`exec` requires an explicit rule |
| `twilio` | `twilio()`, `new Twilio()` | `messages.create`, `calls.create` (other) |
| `@sendgrid/mail` | default export | `send`, `sendMultiple` (email) |
| `@vercel/blob` | — | `put`, `del`, `copy` (file-write) |

A rule applies only when the client provably comes from the package: it is
constructed in the same file by the package's class or factory (awaited or
not), declared with its client type (`client: SupabaseClient`, `queue:
Queue`), exported by the package itself, or imported from a project module
that constructs it (`export const resend = new Resend(key)`) or from a
project function that returns such a construction or declares the client
type (`export function getQueue(): Queue`), through re-exports. A name alone
never matches. Recognition evidence says `built-in rule`, and its strength
is the same as an `importedFrom` configuration match.

A sample with several receivers: [`fixtures/midwrite.json`](../../fixtures/midwrite.json).

## Factory receivers and resolved results

An external receiver can end in a zero-argument call, just like a database
receiver: `{"receiver":"getAnalytics()","method":"insert","category":"other"}`.
Calls with arguments do not match that receiver. Explicit function rules
also match local declarations and imported aliases; `importedFrom` still
checks the exact import specifier.

`resolvedResult` declares the value returned when a configured operation
resolves successfully. It accepts primitive JSON values or a record of up
to 16 primitive properties. For example:

```json
{"receiver":"mailer","method":"send","category":"email","resolvedResult":{"status":true}}
```

After awaiting this operation, `const { status } = ...; if (!status)` has
an unreachable failure arm. A crash between an earlier database commit and
the operation remains a hazard. Rejections and throws are not constrained.
This is an asserted API contract, recorded in recognition evidence; it is
not independently verified and must be supported by the API version in use.

`closedWorld` is a separate, opt-in project assumption. A helper's
independent-commit findings are omitted only when every recognized write
in its expanded body uses the same parameter and every observed invocation
passes a transaction for that parameter. Unobserved entry points, mixed
root/transaction arguments, forwarded function values and unmatched call
facts prevent refinement. The default remains conservative for libraries.

An OpenTelemetry tracer obtained from `trace.getTracer(...)` has a built-in
`startActiveSpan(...)` invocation contract when its package provenance is established:
an inline callback runs in its lexical frame, and its local `return` does
not return from the enclosing function. The API's [documented invocation
and return behavior](https://open-telemetry.github.io/opentelemetry-js/interfaces/_opentelemetry_api._opentelemetry_api.Tracer.html)
is the basis; tracing itself is not treated as a consistency effect. The
callback's returned value is currently unknown to the value analysis.

## Narrowing HTTP effects

A broad `fetch` rule also matches read-only downloads and metadata lookups.
Use `httpMethods` to restrict a fetch-style rule to explicit methods:

```json
{ "function": "fetch", "category": "http-request", "httpMethods": ["POST", "PUT", "PATCH", "DELETE"] }
```

The field is optional and requires `category: "http-request"` and a nonempty
list of uppercase method names. The method comes from the second argument's
object literal: a string-literal `method`, or a conditional between string
literals (`exists ? "POST" : "PUT"`), which matches only when every method is
listed. Request method strings are normalized to uppercase. A later spread,
computed key, getter, or dynamic method invalidates the match; a later explicit
literal `method` establishes it again. Without a `method` (no options, or
options that do not set it) the request is a GET when its first argument is a
string or template literal, `new URL(...)`, or a binding declared `string` or
`URL` (or a `const` initialized with one of those); any other target may be a
`Request`, which carries its own method, and stays unknown.

A helper may take the options as a parameter and spread it into the call
(`fetch(url, { ...options, headers })`) or pass it on (`fetch(url, options)`).
Then each call of the helper decides the method from the argument it passes:
`request(url)` is a GET, `request(url, { method: "PUT" })` is a PUT, and a
helper that passes its own parameter on is decided one call further out. This
holds when the parameter is never reassigned, written through or method-called,
has no other use than property reads and that one, and has no default other than
an object literal without `method` (`options: RequestInit = {}`). Inside the
helper, and at a call that passes no literal method, the request stays unknown.

Unmatched calls remain unknown, rather than being declared pure. They may
produce MW9001 around recognized effects. This includes implicit GETs,
`Request` objects without explicit options, options passed through variables,
and dynamic methods passed to helpers. Configure known mutating helpers
separately when their implementation passes options in other ways. A later
unrestricted rule still matches those calls (a helper call is then classified
by that rule, as before).
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

Rules for `recoveryContracts[]`, all checked when the configuration is
loaded:

* `function` is matched **exactly** as diagnostics name it
  (`Class.method`); there are no patterns, so no blanket suppression.
  `file` is optional and disambiguates equal names.
* `covers` lists codes explicitly; a contract never applies to codes it
  was not written for. `retried` may cover MW1001–MW1005; `reconciled` only
  MW1001, MW1004 and MW1005 (reconciliation from database state cannot undo
  an external effect whose database change never committed). MW9001 cannot
  be covered: it is an analysis limitation, not a failure window.
* `evidence` is required (at least 10 characters) and should say where the
  mechanism is configured; `limits` says when it gives up.
* Two contracts for the same function and file are refused.

Effect on a covered finding: it is **still reported**, with the same
fingerprint, location, failure sequence and trace; under `recommended` a
warning becomes `info`, under `strict` and `audit` severity is unchanged; a
`recovery` note (`declaration`, `scope: "function"`,
`source: "configuration"`, `verified: false`, `effectOnFinding`, the cited
`evidence` and `limits`) is attached in text, JSON and SARIF
(`properties.recovery`); a limitation says the contract is not verified
and, for `retried`, that the effect completed before the failure is
performed again (it must be idempotent or its duplicates acceptable).

Recovery is **never inferred from names**: a function called
`retryFailedJobs`, or arguments named `outbox` or `idempotencyKey`, change
nothing (`recovery.rs: recovery_is_never_inferred_from_names`). Nor is it
inferred from `try/catch` around a best-effort call, because real bugs sit
inside `try/catch` too; declare such effects with `bestEffort`.

Where a recovery property comes from decides where it appears:

| `source` | meaning | where |
| --- | --- | --- |
| `sourceCode` | established by supported analysis (a transactional outbox: the state change and the outbox insert on one transaction handle) | implicit: no finding at all |
| `configuration` | asserted in `midwrite.json`, `verified: false` | static reports |
| `runtimeObservation` | observed by a replay harness's recovery scenarios | verification reports only ([replay protocol](../architecture/replay-protocol.md#verification-levels)) |
| `unknown` | anything else (retries, cron jobs, webhook redelivery) | nothing is attached |

A configuration declaration is never displayed as runtime proof, and a
runtime observation never changes a static finding.

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
[`docs/LIMITATIONS.md` § recognition trust model](../LIMITATIONS.md#recognition-trust-model).

## Configuration in other commands

| where | what |
| --- | --- |
| scenario `extraction` | `root`, `tables`, `externals` (`callee`, `channel`, `keyArgument`), `schema`, `transactionRetries`, `maxInlineDepth`, `replicaFactories` |
| `midwrite-contracts.json` | [contracts guide](../user-guide/contracts.md) |
| `midwrite-source.json`, `midwrite-replay.json` | authorized harness manifests: [source verification](../user-guide/source-verification.md) |
| environment | [CLI reference § environment variables](cli.md#environment-variables) |
