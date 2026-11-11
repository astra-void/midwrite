# Contracts: what source code cannot say

Midwrite never infers retries, delivery guarantees, external
idempotency or business invariants from names. Two optional files let you
declare them; everything taken from them is reported as declared and never
as verified.

| file | read by | purpose |
| --- | --- | --- |
| `midwrite.json` | `check`, `trace`, `ir`, `init-replay` | database receivers, transaction methods, external effects, recovery declarations — [configuration reference](../reference/configuration.md) |
| `midwrite-contracts.json` | `discover` (project root, or `--contracts FILE`) | external effects, delivery and retry, invariants, argument values, transaction helpers, replica factories, harness mapping — this page |
| `midwrite-source.json` | `model-check --verify-source` | the authorized source harness — [source verification](source-verification.md) |

## `midwrite-contracts.json`

Unknown fields are errors (`deny_unknown_fields`). The `kind` stays
`midwrite.contracts`, `schemaVersion` 1; later fields (`replicaFactories`,
return-value templates, formulas over generated values) are additive.

```json
{
  "schemaVersion": 1,
  "kind": "midwrite.contracts",
  "description": "optional",
  "externals": [
    { "callee": "chargeCard", "channel": "card-charge", "keyArgument": 1, "dedup": "byKey" }
  ],
  "entries": {
    "src/services/jobs.ts#processRefund": { "delivery": "at-least-once" },
    "src/app/checkout.ts#checkout": { "retry": "on-crash" },
    "src/app/invitations.ts#inviteMember/arg[1]": { "values": { "data.role": "member" } },
    "src/lib/x.ts#handler": { "invocation": "awaited", "preconditions": ["signed in"], "concurrent": false }
  },
  "invariants": [
    { "name": "one-team-per-owner", "entries": ["src/services/teams.ts#createTeam"],
      "property": { "template": "atMostOne", "table": "teams", "key": ["ownerId"] } }
  ],
  "transactionHelpers": [
    { "callee": "withTransactionRetry", "callbackArgument": 0, "optionsArgument": 1, "attempts": 3 }
  ],
  "replicaFactories": [
    { "factory": "packages/db/src/replicas.ts#withReplicas", "primaryArgument": 0,
      "primaryOnlyMethods": ["usePrimaryOnly"], "primaryProperties": ["$primary"],
      "reason": "read from the factory's source" }
  ],
  "harness": {
    "manifest": "midwrite-source.json",
    "entries": { "src/app/invitations.ts#inviteMember/arg[1]": {
      "entry": "inviteMember", "input": { "form": { "email": "$arg0.email" } } } }
  }
}
```

| field | meaning |
| --- | --- |
| `externals[]` | a callee (as written) is an external effect on `channel`; `keyArgument` is its idempotency key; `dedup`: `none` or `byKey` — the provider's promise, not verified |
| `entries.<selector>.delivery` | `at-least-once`: redelivery after a crash or lost acknowledgment; deliveries may overlap |
| `entries.<selector>.retry` | `on-crash`, `on-failure`, `on-failure-or-crash` |
| `entries.<selector>.concurrent` | whether two invocations can overlap (otherwise an assumption) |
| `entries.<selector>.invocation`, `preconditions` | how a wrapper invokes a callback when its source does not say |
| `entries.<selector>.arguments`, `values` | arguments, or values of single parameter paths, instead of synthesized ones |
| `invariants[]` | business rules from templates `atMostOne`, `atLeast`, `nonNegative`, `maxExternal`, `formula` (a literal [assertion formula](../concepts/invariants.md#assertion-language-scenario-invariants), which can also read return values with `returned`); return-value templates `atMostOneSuccess { function?, returned }`, `exactlyOneSuccess { function? }`, `everySuccessReturns { function?, returned \| argument }` (exactly one of the two); `entries` restricts them. A return-value property is only ever declared: discovery builds a `return-value` candidate for the entries a declaration names, and infers none. Formulas can also state identities of generated values without inventing them (`generatedBy`, `sameGenerated`, `distinctGenerated`, `same`, `forEachActor`, `forEachRow`, `countMatching`/`rowMatching`, `returned`/`argument` selectors; [invariants](../concepts/invariants.md)); see the [templates](#templates-for-multi-row-invariants) below (never declare a rule your application does not require) |
| `transactionHelpers[]` | helpers whose retry loop is bound to the model's transaction retry instead of being analyzed |
| `replicaFactories[]` | a project-defined `withReplicas`-style factory (`module#export`), its primary argument, primary-only methods and properties, and a required `reason`; a declaration that does not resolve is reported and not applied ([source extraction](../architecture/source-extraction.md)) |
| `harness` | the source-harness manifest and, per entry, its input with `$argN` / `$argN.path` placeholders |

Selectors are `file#name`, with `/arg[N]` for a callback argument of a
wrapped function (for example a Server Action wrapped by a validation
helper); see [function identity and source selectors](../architecture/source-extraction.md#function-identity-and-source-selectors).

## Effect of a declaration

* A declared invariant replaces heuristic hypotheses on the same table and
  gives its counterexamples the origin `USER_DECLARED`.
* `delivery` and `retry` enable the duplicate-processing,
  external-effect-gap and resubmitted-write families and the response step
  for actors retried after a crash.
* Write skew and other multi-row rules are never inferred: such a candidate
  stays `BLOCKED` until an invariant is declared, and its next step prints
  a declaration skeleton.
* A declaration is an assumption the model uses. Original-code
  verification *establishes* inputs and preconditions for its run; it does
  not prove they hold for real requests.

## Templates for multi-row invariants

Templates, not findings: each states a business rule the code does not
state, and is correct only if it is your rule. Copy an object into
`invariants`, adjust names, values and entries, and rerun `discover
--model-check --contracts FILE`. The test
`every_contract_template_parses_and_loads_against_the_first_party_application`
(`crates/midwrite-discover/tests/generated_ids.rs`) loads these shapes
against [`fixtures/apps/genid`](../../fixtures/apps/genid).

A group keeps at least one member with a role (write skew of two
concurrent "leave" operations):

```json
{ "name": "team-keeps-an-owner",
  "entries": ["src/teams.ts#leaveTeam", "src/teams.ts#deleteTeamMember"],
  "property": { "template": "atLeast", "table": "users_on_team",
                "groupBy": ["teamId"], "where": { "role": "owner" }, "min": 1 } }
```

`atLeast` is instantiated per group of the synthesized initial rows; a
group that violates it initially is not checked, with a note.

A row's reference names a matching row of another table:

```json
{ "name": "current-team-is-a-membership",
  "entries": ["src/teams.ts#leaveTeam"],
  "property": { "template": "formula", "assert": { "forEachRow": { "table": "users", "assert": { "or": [
    { "eq": [{ "field": "teamId" }, null] },
    { "ge": [{ "countMatching": { "table": "users_on_team",
        "match": { "userId": { "field": "id" }, "teamId": { "field": "teamId" } } } }, 1] } ] } } } } }
```

Declare a formula only for entries whose scenario contains every table it
reads; otherwise the loader refuses it (`unknown table`, verdict
`NOT_LOADED`).

Every completed job reports a stored, generated id (`returned` over a
generated value):

```json
{ "name": "job-reports-a-stored-insight",
  "entries": ["src/jobs/insights.ts#GenerateInsights/method[process]"],
  "property": { "template": "formula", "assert": { "forEachActor": { "assert": { "implies": [
    { "ne": [{ "returned": { "path": "insightId" } }, null] },
    { "eq": [{ "countMatching": { "table": "insights",
        "match": { "id": { "returned": { "path": "insightId" } } } } }, 1] } ] } } } } }
```

No invocation fails: `{ "template": "formula", "assert": { "eq": [{
"actors": { "outcome": "failed" } }, 0] } }`. A rule a unique constraint
already enforces needs no declaration (it is `SCHEMA_ENFORCED`).

Real examples: [`fixtures/eval/p13/eval-app/commitrift-contracts.json`](../../fixtures/eval/p13/eval-app/commitrift-contracts.json),
[`corpora/contracts/midday.json`](../../corpora/contracts/midday.json).
