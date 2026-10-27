# `commitrift-contracts.json`

Optional, in the project root (or `--contracts FILE`). Everything taken
from it is reported as declared. Unknown fields are errors.

```json
{
  "schemaVersion": 1,
  "kind": "commitrift.contracts",
  "externals": [
    { "callee": "chargeCard", "channel": "card-charge", "keyArgument": 1, "dedup": "byKey" },
    { "callee": "sendPayout", "channel": "payout", "dedup": "none" }
  ],
  "entries": {
    "src/services/jobs.ts#processRefund": { "delivery": "at-least-once" },
    "src/app/checkout.ts#checkout": { "retry": "on-crash" },
    "src/app/invitations.ts#inviteMember/arg[1]": { "values": { "data.role": "member" } },
    "src/lib/x.ts#handler": { "invocation": "awaited", "preconditions": ["signed in"], "concurrent": false }
  },
  "invariants": [
    {
      "name": "manager-on-duty",
      "entries": ["src/services/orders.ts#endShift"],
      "property": { "template": "atLeast", "table": "shifts", "groupBy": ["storeId"], "where": { "onDuty": true }, "min": 1 }
    }
  ],
  "transactionHelpers": [
    { "callee": "withTransactionRetry", "callbackArgument": 0, "optionsArgument": 1, "attempts": 3 }
  ],
  "harness": {
    "manifest": "commitrift-source.json",
    "entries": {
      "src/app/invitations.ts#inviteMember/arg[1]": {
        "entry": "inviteMember",
        "input": { "userId": "$arg1.id", "form": { "email": "$arg0.email", "role": "$arg0.role" } }
      }
    }
  }
}
```

| field | meaning |
| --- | --- |
| `externals[]` | a callee (as written) is an external effect on `channel`; `keyArgument` is the idempotency key; `dedup`: `none` or `byKey` (the provider's promise — not verified) |
| `entries.<selector>.invocation`, `preconditions` | how a wrapper invokes a callback when its source does not say |
| `entries.<selector>.delivery` | `at-least-once`: redelivery after a crash or a lost acknowledgement, deliveries can overlap |
| `entries.<selector>.retry` | `on-crash`, `on-failure`, `on-failure-or-crash` |
| `entries.<selector>.concurrent` | whether two invocations can overlap |
| `entries.<selector>.arguments`, `values` | arguments, or values of single parameter paths, instead of synthesized ones |
| `invariants[]` | business rules: `atMostOne`, `atLeast`, `nonNegative`, `maxExternal`, `formula`; `entries` restricts them |
| `transactionHelpers[]` | helpers whose retry loop is not analyzed |
| `harness` | the source-harness manifest and, per entry, its input with `$argN` / `$argN.path` placeholders |
