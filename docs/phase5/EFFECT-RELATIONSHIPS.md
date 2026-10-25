# Effect relationships

Code: `crates/commitrift-core/src/absint/relation.rs`,
`crates/commitrift-core/src/analysis/render.rs` (`value_relation`).

## Question

A finding says "a failure between A and B leaves A done and B not". Phase 4
could not say whether A and B concern the same data; its `relationship`
field compares argument *names* in one function and is "not comparable"
across calls. Phase 5 adds `valueRelation`, computed from the
[provenance](ABSTRACT-DOMAIN.md#provenance) of both operations' arguments,
expressed in the analyzed function's frame (so it works across summarized
calls and returns).

## Classification

| `status` | Condition |
| --- | --- |
| `provenShared` | the second operation's arguments contain, exactly, the result of a first operation (`derivedFromResult`), or both use the same value (or one a property of the other) exactly (`sameValue`) |
| `possiblyRelated` | a shared value only through a computation (template, arithmetic, nested call argument) |
| `noRelationEstablished` | every argument value of both is known and nothing is shared — **not** a proof of independence |
| `unknown` | some argument value is unknown (escaped object, unknown call result used in a computation, captured variable, spread, …) |
| `provenDisjoint` | reserved; **never produced**: database predicates are not interpreted, so different rows can never be established |

For CP1003 the operation performed first is related to the other. Each
shared value lists both sides (`result of `db.insert(orders)`.0.id`,
`param#0.id` — `param#i` is the i-th parameter, zero-based, of the analyzed
function).

```json
"valueRelation": {
  "status": "provenShared",
  "shared": [{ "kind": "derivedFromResult",
               "first": "result of `db.insert(orders)`",
               "second": "result of `db.insert(orders)`.0.id",
               "exact": true }]
}
```

Different variable names never mean different values
(`tests/phase5.rs::shared_inputs_are_the_same_value_and_different_names_are_not_disjoint`),
and a shared tenant id is reported as shared even though it may not mean
the operations belong to one invariant.

## Where it appears

* diagnostics (JSON, SARIF properties unchanged): optional `valueRelation`;
* failure traces: optional `valueRelation` (trace report **schema v2**,
  `docs/phase5/schemas/trace-report.v2.schema.json`; v1 reports still load);
* corpus reports: per finding and as counts per project.

Neither the fingerprint nor the trace id depends on it.

## Policy: descriptive only

Relationships never remove or demote a finding. On the reviewed corpus
findings (`reports/phase5/corpus/relations-vs-labels.txt`; labels by one
reviewer plus an automated second opinion, not independent human review),
the status does not separate the classes:

| label | provenShared | possiblyRelated | noRelationEstablished | unknown |
| --- | --- | --- | --- | --- |
| true positive | 4 | 3 | 4 | 16 |
| intended risk | 13 | 6 | 9 | 37 |
| false positive | 1 | 1 | 0 | 3 |
| unclear | 0 | 0 | 0 | 6 |

Four "true positives" have no established relation and one false positive
shares a value, so no severity rule is justified. Across all findings the
status is `unknown` for most midday findings (139 of 192 with a relation):
arguments are usually built from parameters' properties passed through
unknown calls or captured by callbacks.
