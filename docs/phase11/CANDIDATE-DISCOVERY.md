# Candidate discovery, ranking and deduplication

`commitrift-discover/src/detect.rs`. Each detector is a rule over one
entry's effect summary. A detector **records** a recognized protection (a
unique constraint, a row lock, a conditional update, a deduplicating
channel) on the candidate instead of dropping it, so the model checker
decides whether the protection works; it **rejects** a combination only for
a reason it writes into `rejected`.

## Detectors

| class | rule (all on the control-flow graph) | property (origin) | environment |
| --- | --- | --- | --- |
| `check-then-act` | a read with equality conjuncts; an insert into the same table reachable from it and decided by a guard that reads it; at least one key column whose inserted value is the checked value | at most one row per key (`HEURISTIC_HYPOTHESIS`; `SCHEMA_ENFORCED` when a non-partial unique set covers the key) | 2 actors, same arguments |
| `status-transition` | a read of a row; a guard that tests a state column against a literal, or tests existence when the read's WHERE fixes the state; an unconditional UPDATE of that row setting the state to another literal; inserts or external effects decided by the same guard | at most one consequence row per transition, at most one effect per transition (`HEURISTIC_HYPOTHESIS`) | 2 actors, same arguments |
| `read-modify-write` | a read; an UPDATE of the same row writing a column computed from the read's value (not a column-relative expression) | the cell equals a serial execution of the requests that completed (`HEURISTIC_HYPOTHESIS`) | 2 actors, same arguments |
| `write-skew` | a read of a predicate that is not one row by primary key; a guard on it deciding a write of another row (argument-chosen key) that changes a column of the predicate | **declared only** (`USER_DECLARED`); without one the candidate is `BLOCKED` | 2 actors, different rows of one group |
| `duplicate-processing` | an entry with declared at-least-once delivery and external effects | at most one effect per message (`FRAMEWORK_CONTRACT`) | 2 deliveries; 1 crash, 1 redelivery |
| `external-effect-gap` | an external effect followed by database writes, with a declared retry | at most one effective effect per request (`HEURISTIC_HYPOTHESIS`) | 1 actor, crash + retry |
| `external-effect-gap` (retried transaction) | an external effect inside a transaction the code re-runs (helper attempts > 1) | at most one effect per request (`HEURISTIC_HYPOTHESIS`) | 2 actors (to cause 40001/40P01) |
| `ambiguous-commit` | an autocommit write in a `try` block and the same write (same values) in its `catch` handler | at most one row per key (`SCHEMA_ENFORCED` with a covering unique set, else `HEURISTIC_HYPOTHESIS`) | 1 actor, one lost acknowledgment |
| `lock-order` | two row-locking statements on one table in one transaction, keyed by different parameters | no request fails (`HEURISTIC_HYPOTHESIS`, bounded liveness) | 2 actors, the two keys swapped |

Declared invariants of the contracts replace hypotheses on the same table.

### Rejections

Recorded with location: a check and an insert with no argument-dependent key
in common; a transition whose conditional update's row count is checked; a
repeated transition with no observable consequence; an external effect
followed by writes without a declared retry ("a crash alone repeats
nothing"); a partner whose inserted literal differs from the check's
literal filter (provably distinct rows, e.g. statically different tenants);
entries declared non-concurrent.

### Concurrency is an assumption

Two invocations overlapping is: declared (`concurrent`, `delivery`),
`framework` for Server Actions and route handlers (double submit, two tabs,
client retry), or `discovery-default` for any other exported function ("NOT
established from source"). The assumption is part of every multi-actor
candidate.

## Pairing

For each check-then-act and read-modify-write candidate, partners are the
other entries that write the same `(table, column)` (from the resource
index), excluding helpers other entries inline, each unordered pair once,
at most `--max-partners` (default 6; the rest counted as
`partnersTruncated`). A pair keeps the first entry's property and adds the
partner's operation.

## Feasibility filtering

Kept from the candidate's own facts: reachability on the control-flow graph
(mutually exclusive branches never form a check and an act), literal
filters (provably distinct rows are rejected), and declared non-concurrency.
Everything else — whether the schedule exists, whether a lock or a unique
constraint protects — is left to the model checker on a synthesized state.

## Ranking

Deterministic, explained, without a numeric confidence:

1. status: `MODEL_COUNTEREXAMPLE`, `STATIC_CANDIDATE`, `INCONCLUSIVE`,
   `NO_VIOLATION_WITHIN_BOUNDS`, `UNSUPPORTED`, `BLOCKED`;
2. strongest property origin (`SCHEMA_ENFORCED` … `HEURISTIC_HYPOTHESIS`);
3. no unsupported operation on the path;
4. exposure: request handlers and entries with declared contracts first;
5. fewer entries, fewer assumptions;
6. fingerprint.

`ranking` lists the factors for each candidate. Candidates beyond
`--max-candidates` (in the static order) are reported as
`STATIC_CANDIDATE` without scenarios and counted.

## Identity and deduplication

* **Fingerprint** (stable): class, the actors' selectors, the resource,
  property templates with origins, the environment (relation, retry,
  crashes, in-flight), the conflict roles and operations — no line numbers.
  The candidate id is `CR-SYNTH-` + its first six hex digits.
* **Group** (same underlying issue): class, resource table and columns, and
  the roles and locations of the first entry's operations. A pair and the
  first entry's own candidate share a group; the later one gets
  `duplicateOf`. `summary.deduplicatedFindings` counts groups with a model
  counterexample.
* Candidates differing in isolation, logical key, failure mechanism,
  assumptions, source location, or actual versus hypothetical schema have
  different fingerprints.

## Explanation

Every candidate reports which entries interact (`entries`, with how each
is invoked), which effects conflict (`conflict`, role, operation,
location), the shared resource, each property with origin and rationale,
recognized protections, assumptions with origin, unsupported operations
and whether one is on the path, the environment and bounds of each
scenario, and `inconclusiveIf` (what would make a result inconclusive).
