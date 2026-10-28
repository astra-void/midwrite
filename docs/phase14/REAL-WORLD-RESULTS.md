# Phase 14: real-world results

Static discovery with bounded model checking on the corpus (nothing
executed), and original-code verification on first-party fixtures only.
Raw data: `reports/phase14/corpus/` (`scripts/phase14-corpus.py`: Phase 13
final binary vs Phase 14 binary, same machine and inputs, median of 3 runs,
peak RSS the maximum), `reports/phase14/evidence/`.

## Corpus

| project | config | effectful | read-only | resources | unknown ops | candidates | model-checkable | statuses |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| saas-starter | Phase 13 → 14 | 8 → 9 | 4 → 11 | 17 → 23 | 284 → 237 | 2 → 2 | 1 → 1 | 1 MODEL_COUNTEREXAMPLE (heuristic, Phase 11), 1 UNSUPPORTED — unchanged |
| midday, no declaration | Phase 13 → 14 | 0 → 0 | 0 → 0 | 0 → 0 | 17 696 → 18 954 | 0 → 0 | 0 | — |
| midday, declared replica factory | Phase 13 → 14 | **47 → 81** | 64 → 63 | **52 → 123** | 17 496 → 18 861 | 3 → 4 | **0 → 0** | 1 INCONCLUSIVE, 3 BLOCKED (Phase 13: 1 UNSUPPORTED, 2 BLOCKED) |
| school-ledger (private) | Phase 13 → 14 | **21 → 70** | 30 → 27 | **18 → 68** | 14 615 → 13 396 | 9 → 9 | **0 → 0** | 7 INCONCLUSIVE, 2 UNSUPPORTED (same counts; blockers below) |
| unnamed-ai-project (private) | Phase 13 → 14 | 3 → 3 | 6 → 7 | 3 → 3 | 1 009 → 881 | 0 → 0 | 0 | — |

midday's statements with the declared factory: primary writes 54 → 98
(pool `DELETE` 42, `INSERT` 26, `UPDATE` 30), replica reads 86 → 101
(99 `SELECT`, 2 counts), transactions 16 → 19. Unknown operations rose
because code that was not reached before (behind closures, validated input,
arrays and strings) is now reached and exposes its own unsupported
operations — the same effect Phase 13 documented.

## Slice 1: midday's replica scenario

```text
midday 51587319 ─► declared replica factory (corpora/contracts/midday.json)
 ─► discovery ─► synthesized scenario (replicaFactories, features replicas + js-values)
 ─► loader re-extracts: round trip passes ─► solo runs reach the read and the write
 ─► nanoid(21) is a generated value ─► property instantiation
```

| candidate | Phase 13 final | Phase 14 | first blocker now |
| --- | --- | --- | --- |
| `toggleCustomerPortal` (read-modify-write; replica read, primary update) | UNSUPPORTED (`nanoid` unanalyzed) | **INCONCLUSIVE** | the scenario loads and both actors reach the replica `SELECT` and the primary `UPDATE`; the heuristic serial-outcome property cannot be stated, because every serial outcome of `portalId` is a generated value (`nanoid(21)`), which an invariant term cannot express |
| `leaveTeam`, `deleteTeamMember` (write-skew) | BLOCKED | BLOCKED | no declared invariant (never inferred) |
| `GenerateInsightsProcessor.process` (write-skew, new) | — | BLOCKED | no declared invariant |

During Phase 14, an intermediate build produced a counterexample for this
candidate: a serial outcome that was a generated value had been turned
into `null`, so any real outcome "violated" it. That was a false
counterexample from a defect, fixed in Phase 14 (`synth.rs` `value_term`);
it is not claimed. **Model-checkable midday candidates: 0** (Phase 13: 0).
Replica semantics stay the Phase 13 model (one asynchronous replica,
unbounded lag); no replication was run.

## School-ledger: the nine candidates (aggregate, generic causes only)

All nine are check-then-act with a HEURISTIC_HYPOTHESIS property (unchanged
origin; never upgraded).

| | Phase 13 blocker | newly supported on the path | Phase 14 first blocker | synthesis | model checking |
| --- | --- | --- | --- | --- | --- |
| 7 candidates | inlined authorization guard denies: the synthesized argument lacks the field it tests | closures, zod, arrays and strings now extract (effectful entries 21 → 70) | the same guard: the argument still lacks the tested field | INCONCLUSIVE (no valid initial state) | not reached |
| 2 candidates | template literal with an interpolated package call ("string concatenation not modeled") | template literals; `randomUUID()` as a generated value | a string method applied to the generated value (its text cannot be represented) | UNSUPPORTED | not reached |

No counterexample was forced and none is claimed. With a declared value for
the guard's field (Phase 13's probe), the next blocker was
`Array.prototype.includes` on a field — now modeled; that probe was not
repeated in Phase 14 (it needs a private, uncommitted contracts file).

## Original-code verification (first-party, `fixtures/phase14/request-app`)

PostgreSQL 17.11, disposable clusters, `--verify-source --allow-execution
--source-repeat 3`; `scripts/phase14-evidence.py`.

| slice | model | original code | evidence |
| --- | --- | --- | --- |
| 3 multi-row INSERT, crash before the response, client resubmits (`addOrderItems`) | counterexample | **3/3** reproduced: duplicate order lines | `original-code-crash-reproduced` |
| 3 safe (`addOrderItemsIdempotent`, unique key + DO NOTHING) | no counterexample | — | — |
| 4 retry wrapper, COMMIT acknowledgment lost, callback re-run (`settlePayment`) | counterexample | **3/3** (5/5 in an earlier run) reproduced: two payments | `original-code-connection-loss-reproduced` |
| 4 safe (`settlePaymentOnce`, conditional UPDATE gate) | no counterexample | — | — |
| 4 with the default harness (no client error handler) | counterexample | **diverged** 3/3: the worker died on node-postgres's unhandled client `'error'` | kept in `reports/phase14/evidence/default-harness/` |

The divergence is a finding about the model's assumption, not a defect
claim: the model assumes a broken connection leaves the process alive; a
Node server using node-postgres transactions without a client error handler
dies instead, so its retry never runs. The reproduced runs state the
assumption (the harness handles client errors).

The Phase 13 first-party slices still reproduce with the Phase 14 binary
(full test suite: `crates/commitrift-cli/tests/phase13.rs`), as does the
Phase 12/13 foreign-key-lock slice, now also under parallel load.
