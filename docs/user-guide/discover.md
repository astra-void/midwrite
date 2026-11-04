# `midwrite discover`: candidates and scenarios from source

`discover` finds concurrency and failure candidates in the original
TypeScript without a hand-written scenario. It indexes entries and their
database and external effects, pairs effects that may interact, attaches
property candidates with a stated origin, synthesizes small valid initial
states, writes ordinary `model-check` scenarios and, with `--model-check`,
explores them within budgets. It never executes application code.

Introduced in Phase 11 ([final report](../phase11/FINAL-REPORT.md)),
extended in Phase 12 (generated keys, multi-module schemas, repositories,
shared project context) and Phase 13 (foreign keys, return values, the
response step, read replicas, upserts, row-lock strengths). Internals:
[scenario synthesis](../architecture/scenario-synthesis.md). All flags:
[CLI reference](../reference/cli.md#discover).

## Typical use

```bash
midwrite discover .                                     # static index + candidates + synthesized scenarios
midwrite discover . --model-check                       # also explore them (bounded)
midwrite discover . --model-check --format html -o discovery.html
midwrite discover . --emit-scenarios ./midwrite-scenarios
midwrite discover . --contracts midwrite-contracts.json --entry src/app/checkout
midwrite discover . --cache .midwrite-cache.json      # cache extractions between runs
```

Exit status: `1` when at least one model counterexample was found, `0`
otherwise (errors: `2`).

## Detector families

| class | what it looks for | property (origin unless declared) |
| --- | --- | --- |
| `check-then-act` | a read with equality conjuncts, then an insert into the same table decided by it, sharing a key column | at most one row per key (`HEURISTIC_HYPOTHESIS`; `SCHEMA_ENFORCED` when a non-partial unique set covers the key) |
| `status-transition` | read a row, test its state, unconditional `UPDATE` to another state, consequences under the same guard | at most one consequence / effect per transition |
| `read-modify-write` | `UPDATE` of a column computed from a value read earlier | the cell equals some serial execution |
| `write-skew` | a predicate read deciding a write of another row of the group | **declared only**; otherwise `BLOCKED` |
| `duplicate-processing` | declared at-least-once delivery with external effects (Phase 14: also the inserts a declared invariant covers) | at most one effect per message (`FRAMEWORK_CONTRACT`); declared invariants on the inserted tables |
| `external-effect-gap` | external effect followed by writes, with a declared retry (or inside a transaction the code re-runs) | at most one effective effect per request |
| `ambiguous-commit` | an autocommit write in `try` repeated in its `catch`; Phase 14: also a write in a transaction whose COMMIT is in the `try` (a retry wrapper's callback) | at most one row per key |
| `lock-order` | two row locks on one table in one transaction keyed by different parameters | no request fails (bounded liveness) |
| `resubmitted-write` (Phase 12) | an entry with a declared crash retry whose `INSERT` (Phase 14: also a multi-row `INSERT`) can commit before the crash | only with a declared invariant on the table or a covering unique constraint; otherwise a recorded rejection |
| `parent-child` (Phase 14) | a read finds a parent row by a unique key and decides an `INSERT` of a child holding that key, while another entry deletes the parent | no orphan child: `SCHEMA_ENFORCED` with a `NO ACTION` foreign key (a 23503 is not an application defect), `HEURISTIC_HYPOTHESIS` without one |
| `return-value` (Phase 14) | an entry named by a **declared** property over what callers are told (`atMostOneSuccess`, `exactlyOneSuccess`, `everySuccessReturns`, or a formula reading `returned`) | the declared property only; never inferred |

Source: [Phase 11 candidate discovery](../phase11/CANDIDATE-DISCOVERY.md),
[Phase 12 architecture § 5](../phase12/ARCHITECTURE.md#5-discovery-changes).
Phase 13 changed two rules: only `FOR UPDATE` and `FOR NO KEY UPDATE` count
as a protection (two `FOR SHARE` readers do not exclude each other), and
actors whose caller retries after a crash get the response step
([Phase 13 advanced Drizzle](../phase13/ADVANCED-DRIZZLE.md#2-row-lock-strengths--implemented)).

## Reading a candidate

Each candidate reports its entries (and how each is invoked), the
conflicting operations with locations, the shared resource, every property
with its origin and rationale, recognized protections, assumptions,
unsupported operations on the path, and the scenarios built for it
(`actual`, a `distinct-key` control, and with `--hypothetical` a schema the
application does not have). A real excerpt is in
[first verification](../getting-started/first-verification.md#2-discover-candidates-without-writing-a-scenario).

| status | meaning |
| --- | --- |
| `MODEL_COUNTEREXAMPLE` | the `actual` scenario has a counterexample that replays in the model; interpret it by the property's origin |
| `NO_VIOLATION_WITHIN_BOUNDS` | no counterexample within the scenario's bounds; not a proof |
| `STATIC_CANDIDATE` | a scenario was built but not model-checked (no `--model-check`, or beyond `--max-candidates`) |
| `INCONCLUSIVE` | no valid initial state could be synthesized, or a budget ran out |
| `UNSUPPORTED` | an actor reaches an operation the model does not represent; the reason and location are printed |
| `BLOCKED` | the property must be declared (for example write skew) |

A counterexample against a `HEURISTIC_HYPOTHESIS` is reported as such and
is never a confirmed defect ([invariants](../concepts/invariants.md)).
Concurrency itself is an assumption: declared, `framework` (Server Actions,
route handlers), or `discovery-default` for any other exported function.

## Declaring what source cannot say

Retries, at-least-once delivery, external idempotency, business invariants,
argument values, read-replica factories and harness inputs go into an
optional `midwrite-contracts.json`
([contracts guide](contracts.md)). Everything taken from it is reported as
declared.

## From a candidate to runtime evidence

Emitted scenarios carry a `sourceVerification` section when the contracts
map the entry to an authorized harness. Running them needs the separate,
explicit `model-check --verify-source --allow-execution`
([source verification](source-verification.md)); `discover` itself never
runs code.

Phase 14: every candidate ends with a **next step** (`nextStep` in JSON,
"Next step:" in text and HTML); a synthesized scenario is checked to
re-extract exactly what discovery extracted (round trip) before it is
model-checked; discovery extracts with JavaScript value semantics and opts
its scenarios into the `js-values`, `timestamps` and `json` features when
they need them ([Phase 14](../phase14/FINAL-REPORT.md)).

## Input exploration (Phase 15)

With `--model-check`, each actual variant is checked in up to
`--max-worlds` (default 8) **initial worlds**: the synthesized one, then
worlds that each change one input relationship — a nullable argument set
to `null` (only where the TypeScript annotation admits it), an argument at
a branch's operand or boundary, a stored value distinct from (or equal to)
the value an actor writes, a stored value at the boundary of a guard, a
non-key argument that differs between the actors. The search stops at the
first validated counterexample (`--all-worlds` continues). Guards that
synthesized inputs failed (an admin role, a caller kind) are satisfied
with **stated** preconditions ("SYNTHESIZED precondition … not assumed of
every caller").

Every model-checked candidate has a **coverage** section, kept in four
dimensions — source semantics, initial states, actor inputs, schedules —
plus branch outcomes and the list of worlds and gaps
(`--explain-coverage` prints them). A `NO_VIOLATION_WITHIN_BOUNDS` result
always says how many worlds were explored and that the input domain was
not exhaustively explored. Details:
[Phase 15 verdict semantics](../phase15/VERDICT-SEMANTICS.md).

```text
  Input exploration (relationship-search): 3 world(s) model-checked, controls included — actual configuration: 1 distinct row set(s), 2 distinct argument vector(s)
    world `base` (actual, Base): explored — NO_COUNTEREXAMPLE_WITHIN_BOUNDS
    world `null:parentId` (actual, Domain): explored — COUNTEREXAMPLE_FOUND
  Schedule exploration (sleep-sets): completed for 2 world(s), truncated for 0, unsupported for 0
  Branch outcomes reached alone: 5 of 8
  Input domain exhaustive: no (every derived relationship class was explored)
  Status: MODEL_COUNTEREXAMPLE
  In the initial world `null:parentId` (parentId = null for every actor): MODEL_COUNTEREXAMPLE: …
```

## Known blind spots

Phase 16: on its fresh frozen set (40 cases), first scoring and final
both precision 1.000, recall 0.333. The most frequent causes of misses are
JavaScript standard-library calls (`Set.prototype.has`,
`Number.isSafeInteger`, `Math.min`) and status transitions keyed by the
id of a row read first. On real projects: zod refinements (`.superRefine`),
optional chains on call results, `Promise.all` with a used result
([Phase 16 evaluation](../phase16/PRECISION-RECALL.md),
[real-world coverage](../phase16/REAL-WORLD-COVERAGE.md)). Phase 16 lifted:
zod `strictObject` and string formats, bounded loops, request bodies,
hoisted predicates and multi-row `FOR UPDATE` (below).

Phase 15: on its fresh input-domain set, first scoring recall 0.308,
post-feedback 0.615, precision 1.000; the Phase 13 set's E26 and the
Phase 14 set's E21 are now found
([Phase 15 evaluation](../phase15/PRECISION-RECALL.md)). Still not
covered (Phase 15): zod `strictObject` and string formats, loops,
request bodies, time arithmetic, predicates hoisted into locals, multi-row
`FOR UPDATE`. Earlier:
Recall on the Phase 14 evaluation set is still low (first scoring 0.000,
post-feedback 0.222, precision 1.000; on the Phase 13 frozen set 0.467,
post-feedback; [Phase 14 evaluation](../phase14/EVALUATION.md)). Not covered:
computed keys, loops other than `for … of` over a known array, `values(obj)`
of a parameter object, framework request objects (`req.json()`), time
arithmetic, JSON projections and comparisons, a stale replica read without a
declared property, and return-value properties without a declaration. On the real corpus no candidate has been model-checked to a
counterexample except saas-starter's heuristic one (Phase 11, never
executed). See [limitations](../reference/limitations.md).
