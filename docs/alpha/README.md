# Midwrite Closed Alpha: invited-developer package

This package is for developers invited to try Midwrite on
TypeScript + Drizzle + PostgreSQL code. It is **not** a public release:
nothing is published to a package registry, and this guide is not
distributed automatically. Every command below is run end to end by
`scripts/phase16-alpha-check.sh` (last result: see
`docs/phase16/ALPHA-READINESS.md`).

| document | for |
| --- | --- |
| this guide | build, first run, reading a result, reproducing it |
| [feedback template](feedback-template.md) | what you tried, what was useful, what was wrong |
| [bug-report template](bug-report-template.md) | a wrong result or a crash |
| [security instructions](../reference/security.md) | what may execute code, and how to keep that contained |
| [supported patterns](../reference/supported-patterns.md) / [limitations](../reference/limitations.md) | what the alpha can and cannot model |
| [evidence levels](../concepts/evidence-levels.md) | what each result does and does not establish |

## 1. What Midwrite does (and does not)

Midwrite reads your server code, finds operations that could go wrong
under concurrency, retries, crashes or replica lag (check-then-act,
read-modify-write, lock order, retries after an unknown outcome, …),
builds small initial databases and inputs, and **model-checks** every
interleaving of two requests within explicit bounds against a model of
PostgreSQL. It never runs your code unless you pass
`--verify-source --allow-execution` with a harness you wrote.

A result is one of:

| status | meaning |
| --- | --- |
| `MODEL_COUNTEREXAMPLE` | a bounded schedule violates the property. If the property is `USER_DECLARED` or `SCHEMA_ENFORCED` it is a rule your code breaks in the model; if it is a `HEURISTIC_HYPOTHESIS`, Midwrite guessed the rule — decide whether you require it |
| `NO_VIOLATION_WITHIN_BOUNDS` | nothing found **within the listed bounds and explored inputs**. Not a proof of safety |
| `INCONCLUSIVE` | no valid initial state or property instance was found |
| `BLOCKED` | the property cannot be inferred and must be declared (Midwrite never invents business rules) |
| `UNSUPPORTED` | the code path uses something the model cannot represent; the first such construct is named |
| `STATIC_CANDIDATE` | found statically, not model-checked (`--model-check` not given) |

## 2. Supported stack (alpha)

TypeScript server functions, Next.js Route Handlers (`request.json()`,
`Response.json`) and plain exported functions; Drizzle ORM (0.45.x
verified) on PostgreSQL 15+; zod 3 and zod 4 object validation (a
documented subset); bounded `for`/`while` retry loops; multi-row
`SELECT … FOR UPDATE`; `UNIQUE … NULLS NOT DISTINCT`; generated
identifiers (`crypto.randomUUID`, `nanoid`, `$defaultFn`); a declared
asynchronous read replica. Details and every refusal:
[supported patterns](../reference/supported-patterns.md).

## 3. Build

Rust 1.97 or newer:

```bash
cargo build --release -p midwrite-cli
```

The binary is `target/release/midwrite`. Node 22 and the PostgreSQL
server binaries are needed only for step 8.

## 4. First run

The example application `fixtures/phase16/retry` places orders in a retry
loop. Run discovery with model checking:

```bash
target/release/midwrite discover fixtures/phase16/retry --model-check
```

Read the first candidate:

* **Entry and source location** — `src/app/api/orders/route.ts#POST`
  and `src/services/orders.ts#placeOrder`; the conflicting statements
  are named with `file:line:col` (`INSERT orders
  src/services/orders.ts:16:13`, the insert inside the retry loop).
* **Property and its origin** — `One client request records at most one
  order.`, origin `USER_DECLARED` (from `midwrite-contracts.json`).
  `recordPayment`'s property is `SCHEMA_ENFORCED` (a `UNIQUE` constraint);
  a `HEURISTIC_HYPOTHESIS` is Midwrite's guess and is always labeled as
  such.
* **Status** — `MODEL_COUNTEREXAMPLE`: when the insert commits but its
  acknowledgment is lost, the loop's next attempt inserts a second order.
  `recordPayment` (same loop, idempotent insert) is
  `NO_VIOLATION_WITHIN_BOUNDS`.

## 5. Coverage: what was and was not explored

```bash
target/release/midwrite discover fixtures/phase16/retry --model-check --explain-coverage
```

Every candidate states its input exploration (which initial worlds and
argument relationships were model-checked; `Input domain exhaustive: no`
— values are never enumerated), its schedule exploration (completed or
truncated per world), and its assumptions, including how each loop was
bounded (`loop BOUNDED_UNROLLING at …`, or `TRUNCATED` when the loop may
run beyond the explored bound).

## 6. Reproduce a counterexample in the model

```bash
target/release/midwrite discover fixtures/phase16/retry --model-check --emit-scenarios out/scenarios
target/release/midwrite model-check out/scenarios/cr-synth-65f47a-actual.json
```

The scenario file is self-contained (schema, initial rows, arguments,
bounds); `cr-synth-65f47a-actual.counterexample.json` records the
violating schedule, the world and the commands. `model-check` prints the
schedule step by step with the source location of each statement.

## 7. Unsupported behavior

```bash
target/release/midwrite discover fixtures/phase16/loops
```

When a path uses something outside the subset, the candidate is
`UNSUPPORTED` and names the first construct and its location (for
example a loop whose bound is not established, `for...in`, a refinement
callback in a zod schema). Unsupported code is never treated as safe.

## 8. Optional: verify on the original code (executes code)

Only with your explicit authorization, on code you trust, against a
disposable PostgreSQL cluster Midwrite creates and deletes:

```bash
cd fixtures/phase16/retry && npm ci && npm run build && cd -
target/release/midwrite model-check out/scenarios/cr-synth-65f47a-actual.json --verify-source --allow-execution --source-repeat 2
```

The harness (`fixtures/phase16/retry/verify/harness.ts`,
`midwrite-source.json`) is written by the developer and says how each
entry is called. Without `--allow-execution` nothing runs. Read
[security instructions](../reference/security.md) first.

## 9. On your own code

1. `midwrite discover <your-app> --model-check` (start without
   contracts; read every `UNSUPPORTED` first blocker and every
   `HEURISTIC_HYPOTHESIS`).
2. Declare the rules your application requires in
   `midwrite-contracts.json` ([contracts](../user-guide/contracts.md);
   templates for generated identifiers:
   [GENERATED-ID-INVARIANTS.md](../phase16/GENERATED-ID-INVARIANTS.md#appendix-contract-templates)).
   Never declare a rule only to obtain a finding.
3. Send what you saw with the [feedback template](feedback-template.md);
   wrong results with the [bug-report template](bug-report-template.md).

## Known limitations that matter in the alpha

* Results are bounded: two concurrent requests, small synthesized worlds,
  loops unrolled at most 8 times. `NO_VIOLATION_WITHIN_BOUNDS` is not a
  proof.
* On the larger real projects measured so far, most candidates still stop
  at an unsupported construct (`docs/phase16/REAL-WORLD-COVERAGE.md`).
* Lock order without `.orderBy(primaryKey)` follows primary-key order in
  the model; PostgreSQL may use another order.
* Replica counterexamples are model-only.
* No independent human review of the evaluation sets has been performed.
