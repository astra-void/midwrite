# Return values, the response step, and crashes before the response (Track B)

Consolidates the plan's `TASK-RETURN-VALUES`, `HTTP-RESPONSE-LIFECYCLE` and
`RESPONSE-LOSS-VERIFICATION`.

## 1. Task outcomes

The plan's conceptual `TaskOutcome` maps onto existing state plus one new
field, so there is no second value system:

| conceptual | represented as |
| --- | --- |
| `Returned(v)` | `Outcome::Ok` and `ActorState.returned` gains `Ok(v)` (`v` a model `Value`: null, bool, int, string, row, generated value) |
| `Threw(e)` / `Rejected(e)` | `Outcome::Failed(reason)` (a JavaScript throw and a rejected awaited promise are the same to the caller) |
| `Terminated` | `Outcome::Crashed` |
| `Unknown` | `ReturnSlot = Err(reason)`: the return expression is not modeled |

`returned` keeps one entry per completed attempt, in order (a retried request
that completes twice told its callers twice). `undefined` is represented as
`null`.

**Propagation.** The extractor already computed the value of a `return` (for
inlined helpers it becomes the helper's result local). For the entry itself
it now emits `Op::ReturnValue { expr }` before `Op::Return` when asked
(`ExtractRequest.return_values`): literals, locals, query results, mutation
row counts (`rows.length > 0` of `update … returning()`), comparisons,
helper results, awaited promises, early returns, expression-bodied arrows.
`Op::ReturnValue` is local (not a scheduling point). An expression that is
not understood stays `Expr::Unknown`; the scenario loader refuses an
invariant that reads such an actor's return value ("the return value at
file:line is not modeled: …") — it never invents one. A JavaScript error in
a well-formed return expression (a property of `null`) fails the task, as
in JavaScript.

**Only when needed.** Return values are recorded only when an invariant
reads them; then they are part of the state (marker `0xD3`). Otherwise
`ReturnValue` is not extracted and states are Phase 12's (test
`return_values_not_read_by_an_invariant_leave_the_state_space_unchanged`).

## 2. Invariants over return values

An `actors` term counts completions; with `"returned": v` only those whose
main task returned `v`:

```json
{ "le": [ { "actors": { "function": "claimTask", "outcome": "ok", "returned": true } }, 1 ] }
```

"At most one caller is told it claimed the task." `returned` needs `outcome:
"ok"`. Rendering: `claimtask:ok returning true`. Old invariants are
unchanged (the field is optional). Discovery can attach such a property only
when it is declared — no heuristic infers that `true` means "claimed" (see
`EVALUATION.md`: the frozen evaluation set declares none, so its four
observable positives — E09, E11, E16 and E23 — are false negatives; E23 is
also blocked by an unmodeled timestamp column. This said "three" until the
closeout).

## 3. The response step

A narrow, explicit request model: an actor with `"response": true` is a
request handler whose response is a separate visible step `Op::Respond`,
inserted by the loader before every `Op::Return` of the main code (jump
targets are kept: a jump to a return reaches its response first). The
conceptual lifecycle maps as:

| conceptual | model | original-code harness |
| --- | --- | --- |
| REQUEST_STARTED, APPLICATION_RUNNING | the task runs | the worker calls the entry |
| APPLICATION_RETURNED, RESPONSE_PREPARED | `ReturnValue`, then the task is at `Respond` | the entry's promise resolved; the worker announces `{"type":"respond","value":…}` and holds |
| RESPONSE_SENT | the `Respond` step | the coordinator releases it (`go`) |
| CLIENT_ACKNOWLEDGED | the completion is recorded (`returned`, `completions`) | the coordinator receives the response (`done`) |

A crash is possible between the last database operation and `Respond`: the
work is committed (its autocommit or COMMIT finished), the process dies, the
client never learns it, and a declared `onCrash` retry runs the request
again. A response lost with the process alive is the existing
`atLeastOnce` retry trigger. Error responses (a thrown handler) are not a
separate step. Framework internals are not modeled: the coordinator plays
the client, and "delivered" means the coordinator received the response.

## 4. Epistemic separation (B7)

Physical database state (the store), process state (crashed / running),
response state (whether `Respond` ran) and the client's observation (the
completion and its return value) stay separate: a request whose INSERT
committed and whose response was lost is `Outcome::Crashed` with the row
committed — never a failed transaction.

## 5. Original-code verification (B8, Vertical slices 2 and 3)

First-party application `fixtures/phase13/request-app` (Drizzle on
node-postgres), authorized harness `commitrift-source.json`, scenarios in
`fixtures/phase13/request-app/scenarios`, disposable PostgreSQL 17.11:

| scenario | model | original code |
| --- | --- | --- |
| `claim-race`: two `claimTask` (read, then unconditional `UPDATE`, `return true`) | counterexample: both callers told `true`; the database state alone is acceptable | **reproduced 3/3**: the invariant evaluated on the values the original functions returned |
| `claim-atomic`: `update … where assignee is null returning()`, `return rows.length > 0` | no counterexample | — |
| `signup-crash`: `recordSignup` (one INSERT with a database-generated key, then `{status: 201}`), crash retry | counterexample: INSERT, crash before the response, retry, INSERT, respond | **reproduced 3/3** (`original-code-crash-reproduced`): the worker is SIGKILLed at the response checkpoint ("before RESPONSE (announced, never sent)"), the retry is a new process |
| `signup-idempotent`: the request id as key, `onConflictDoNothing` | no counterexample | — |

Without the response step the same `recordSignup` scenario has no
counterexample (the Phase 12 false negative); with it, discovery finds it on
the Phase 12 evaluation application too (`CR-SYNTH-E9D6D6 →
MODEL_COUNTEREXAMPLE`).

**A harness defect found here (fixed).** The crash checkpoint's committed
rows contain a generated key that no statement showed (no `RETURNING`); the
coordinator matched generated values against the final rows first, where two
rows differing only in that key are interchangeable, so the crash-time check
disagreed in about half of the runs. Generated values are now matched
chronologically (crash checkpoints before the final rows); 8/8 consecutive
runs agreed afterwards.

## 6. Reductions

`Respond` touches only its actor; `ReturnValue` is local. On the claim,
atomic-claim and signup models the sleep-set search, classic DPOR and
Source-DPOR agree with the unreduced search, and the independence audit is
clean (`model/tests/phase13.rs`).
