# Contracts: what source code cannot say

CommitRift never infers retries, delivery guarantees, external
idempotency or business invariants from names. Two optional files let you
declare them; everything taken from them is reported as declared and never
as verified.

| file | read by | purpose |
| --- | --- | --- |
| `commitrift.json` | `check`, `trace`, `ir`, `init-replay` | database receivers, transaction methods, external effects, recovery declarations — [configuration reference](../reference/configuration.md) |
| `commitrift-contracts.json` | `discover` (project root, or `--contracts FILE`) | external effects, delivery and retry, invariants, argument values, transaction helpers, replica factories, harness mapping — this page |
| `commitrift-source.json` | `model-check --verify-source` | the authorized source harness — [source verification](source-verification.md) |

## `commitrift-contracts.json`

Unknown fields are errors (`deny_unknown_fields`). Format introduced in
[Phase 11](../phase11/CONTRACTS.md); `replicaFactories` added in Phase 13.

```json
{
  "schemaVersion": 1,
  "kind": "commitrift.contracts",
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
    "manifest": "commitrift-source.json",
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
| `invariants[]` | business rules from templates `atMostOne`, `atLeast`, `nonNegative`, `maxExternal`, `formula` (a literal [assertion formula](../phase6/INVARIANTS.md#assertion-language), which can also read return values with `returned`); Phase 14 return-value templates `atMostOneSuccess { function?, returned }`, `exactlyOneSuccess { function? }`, `everySuccessReturns { function?, returned \| argument }` (exactly one of the two); `entries` restricts them. A return-value property is only ever declared: discovery builds a `return-value` candidate for the entries a declaration names, and infers none. Phase 16 formulas can also state identities of generated values without inventing them (`generatedBy`, `sameGenerated`, `distinctGenerated`, `same`, `forEachActor`, `forEachRow`, `countMatching`/`rowMatching`, `returned`/`argument` selectors): see [generated-identifier invariants](../phase16/GENERATED-ID-INVARIANTS.md), whose appendix has contract templates to adapt (never declare a rule your application does not require) |
| `transactionHelpers[]` | helpers whose retry loop is bound to the model's transaction retry instead of being analyzed |
| `replicaFactories[]` | a project-defined `withReplicas`-style factory (`module#export`), its primary argument, primary-only methods and properties, and a required `reason`; a declaration that does not resolve is reported and not applied ([Phase 13](../phase13/REPLICA-AWARE-ANALYSIS.md#4-plumbing-commitrift-discover)) |
| `harness` | the source-harness manifest and, per entry, its input with `$argN` / `$argN.path` placeholders |

Selectors are `file#name`, with `/arg[N]` for a callback argument of a
wrapped function (for example a Server Action wrapped by a validation
helper); see [Phase 7 source identity](../phase7/SOURCE-IDENTITY.md).

## Effect of a declaration

* A declared invariant replaces heuristic hypotheses on the same table and
  gives its counterexamples the origin `USER_DECLARED`.
* `delivery` and `retry` enable the duplicate-processing,
  external-effect-gap and resubmitted-write families and, since Phase 13,
  the response step for actors retried after a crash.
* A declaration is an assumption the model uses. Original-code
  verification *establishes* inputs and preconditions for its run; it does
  not prove they hold for real requests.

Real examples: [`fixtures/phase13/eval-app/commitrift-contracts.json`](../../fixtures/phase13/eval-app/commitrift-contracts.json),
[`corpora/contracts/midday.json`](../../corpora/contracts/midday.json).
