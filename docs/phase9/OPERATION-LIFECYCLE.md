# Operation lifecycle

The lifecycle of an operation that can lose its acknowledgment is specified in
`FORMAL-MODEL.md` §2–3. This note records the part that is easy to get wrong:
**which stages are modeled, which are inferred and which are observed.**

| stage | modeled event | inferred? | runtime-observed by | reported as |
| --- | --- | --- | --- | --- |
| Prepared | the task is at the operation (`candidate`) | no | the worker's announcement (`query` message) | `Release`/`InFlight` plan step matched |
| Submitted | `LostX` / `CrashInFlight` transition | no | proxy `Message` events (frames) | `wireLog`, `inFlight.proxyEvents` |
| ServerReceived | `Orphan` exists (the server holds the operation) or the operation executed | **inferred** for a held COMMIT: the model says the server received it; what the run *observes* is that it was **not yet forwarded** (a held COMMIT is delivered later) | `OperationForwarded` (forwarded), `OperationHeld` (withheld) + `idle in transaction` | `inFlight.forwardedToServer`, `withheldFromServer`, `backendStateBeforeKill` |
| Executing | inside `ServerComplete` | — | — | not separately observed |
| ServerCompleted | `ServerComplete` | no | the server's answer frame; the durable rows | `inFlight.serverAnswer`, `runtimeTables`, `completions[]` |
| ResponsePending | the answer exists, undelivered | no | `ResponseWithheld` | `inFlight.mechanism` |
| ClientAcknowledged | the task continues | no | the worker's `result`/`error` message | `inFlight.clientAcknowledged = false` |
| OutcomeUnknown | `Ambiguity` | **inferred** (the application sent and got nothing) | — | `inFlight.applicationKnowledge` (always "ambiguous") and `physicalOutcome` kept apart |

## Operations covered

| operation | in flight | notes |
| --- | --- | --- |
| autocommit `INSERT`/`UPDATE`/`DELETE` | yes | `Target::Statement`; held or answered |
| explicit `COMMIT` | yes | `Target::Commit` |
| explicit `ROLLBACK` | no | its outcome is the same either way: the transaction does not commit |
| constraint failure, serialization failure, lock-blocked statement | not an in-flight *fault*; they are statement outcomes the model already has (`40001` at COMMIT is a `ServerComplete` that is rejected → world `NotCommitted`) | `a_commit_the_server_rejects_later_is_not_a_committed_world` |

A lock-blocked autocommit statement cannot be an orphan that executes while it
would wait: `ServerComplete` is enabled only when the statement would run
(`orphan_ready`). Whether a backend completes a *blocked* statement of a client
that died is the server's behavior after the lock is released; Phase 9 does
not model that completion.
