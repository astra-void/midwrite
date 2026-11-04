# Phase 16 Track E: invariants over generated identifiers

Generated-identifier-aware invariant terms, serial-outcome properties whose
outcomes are generated values, replica-aware distinctions, the midday
re-evaluation and contract templates for the candidates that need a
declaration. Code: `crates/commitrift-model/src/invariant.rs` (terms,
formulas, validation, evaluation, diagnosis), `dsl.rs` (`generate`),
`witness.rs`/`html.rs` (the diagnosis in reports),
`crates/commitrift-discover/src/synth.rs` (`serial_values` and the
`SerialCell` formula). Tests: `crates/commitrift-model/tests/phase16_genid.rs`,
`crates/commitrift-discover/tests/phase16_genid.rs`. Application:
`fixtures/phase16/genid`.

## 1. Facts this builds on

* A generated value is `Value::Gen(GenId { generator, actor, n })` (Phase 12,
  `docs/phase12/GENERATED-VALUE-SEMANTICS.md`): its identity is the
  generation event; `generator` is the column generator's index in
  `Model::generators()` or `ir::CALL_GENERATOR` (`0x800`) for every package
  call with a fresh-value contract (`nanoid()`, `crypto.randomUUID()`,
  Phase 14); `actor` is the generating actor; `n` is that actor's
  generation counter, shared by all its generators and never reset (a
  rollback, a retried transaction and a retried request all generate new
  events). Equality is identity; ordering is an evaluation error; generated
  values are never initial data. Clock readings (Phase 14, `db::CLOCK_DB`,
  `db::CLOCK_APP`) are also `Value::Gen`.
* A call site is **not** part of a generation event: two `nanoid()` calls
  of one actor differ only in `n`.
* Phase 6-15 invariant comparisons (`eq`, `ne`, `lt`, ...) are false for
  any pair of generated values (`invariant::cmp` falls through), so no
  earlier invariant could state anything about one, and the `SerialCell`
  heuristic refused a serial outcome that was generated ("a serial
  execution leaves gen#2048:0.0 in `customers.portalId`, which an invariant
  cannot state"): midday's portal candidate stayed INCONCLUSIVE.
* Return values (Phase 13) are recorded per completed attempt
  (`ActorState::returned`) only when an invariant reads them
  (`Formula::reads_returned`); a declared formula reading them makes
  discovery build a `return-value` candidate of two concurrent calls with
  equal arguments (`detect.rs` `declared_outcome`).
* The read replica (Phase 13) replays primary commits in commit order by
  optional `Replicate` transitions: lag is unbounded, so a replica read may
  return any committed prefix. Freshness is a property only when declared
  (Phase 14/15 `display-name-read-your-writes`,
  `nickname-read-your-writes`: `formula` templates over return values).

## 2. The language additions

All additions are new enum variants or new structs, so every Phase 6-15
invariant has the same JSON and the same meaning (§6). `T` is a term.

| JSON | kind | meaning |
| --- | --- | --- |
| `{"generatedBy": {"value": T, "by"?: S, "generator"?: G}}` | formula | `T` is a generated **identifier** (not a clock reading) whose generation event belongs to the actor(s) `S` and the generator `G` |
| `{"sameGenerated": [T, T]}` | formula | both are generated identifiers of the **same** event |
| `{"distinctGenerated": [T, T]}` | formula | both are generated identifiers of **different** events (false when either is not generated: not the negation of `sameGenerated`) |
| `{"same": [T, T]}` | formula | both values are present (not `null`/`undefined`) and equal; a generated value equals only itself |
| `{"forEachActor": {"function"?: f, "completed"?: true, "assert": F}}` | formula | `F` holds for every actor running `f` (every actor when absent), binding it as `self`; by default only actors whose main task completed at least once (`"completed": false` includes the others) |
| `{"forEachRow": {"table": t, "where"?: {...}, "assert": F}}` | formula | `F` holds for every committed row of `t` matching the literal filter, binding it for `field` |
| `{"countMatching": M}` / `{"rowMatching": M}` with `M = {"table", "where"?, "match"?: {col: T}, "insertedBy"?: S, "column"?}` | term | number of committed rows (or `column` of the first, in key order; `null` if none) matching the literal `where`, whose column equals each `match` term's value, and that were inserted by `S` |
| `{"returned": {"actor"?: name, "path"?: "a.b"}}` | term | what the actor (`self` when absent) returned in its **latest** completed attempt, or that object's field; `null` before any completion or when the field is absent |
| `{"argument": {"actor"?: name, "path": "p.a"}}` | term | the actor's argument `p` (or its field); how a declared property names the row a call works on without knowing synthesized values |
| `{"field": "c"}` | term | column `c` of the row bound by `forEachRow` |

Selectors: `S` is `"self"` (the innermost `forEachActor`'s actor),
`{"actor": "A"}` (a scenario actor name) or `{"function": "f"}` (any actor
running `f`; an entry selector answers to its export's name as in
`actors`). `G` is `"call"` (any package call with a fresh-value contract)
or `{"column": "table.column"}` (that column's generated default).

### Precise semantics

1. **Identity.** `sameGenerated(a, b)` ⇔ `a = Gen(x)`, `b = Gen(y)`, both
   identifiers, `x = y` (all of generator, actor and counter).
   `distinctGenerated` ⇔ both identifiers and `x ≠ y`. A value is an
   identifier when it is not a clock reading and not a clock placeholder
   (`db::is_clock`, `db::CLOCK_PLACEHOLDER`); clock readings are times, not
   identities, and match none of the identity predicates.
2. **Aliasing.** A generated value copied into several places (a key and a
   foreign key, a local and a row, a returned object) is the same event
   everywhere: `sameGenerated` holds between all copies. `match` compares by
   identity too, so `countMatching` follows a generated foreign key to its
   row (test `rows_are_matched_by_term_values_and_by_their_inserting_actor`).
3. **"Generated by actor A" means any of A's generation events of the
   selected generator — not a specific occurrence.** The occurrence
   counter `n` depends on the number of earlier attempts (a retried request
   or transaction generates again) and on the actor's other generators
   (they share the counter), so the same logical value has different `n` in
   a serial execution without retries and in a concurrent execution with
   them. Matching by actor and generator is invariant under both.
   The price is precision: when A generates two values of one generator
   (two call sites, or two attempts), `generatedBy` accepts either.
   Properties that must distinguish attempts state it through the data
   instead: "the returned id is the id of a row inserted by the same actor"
   (`countMatching` with `match` and `insertedBy`) is false for an id whose
   insert was rolled back or failed, whatever its counter (test
   `a_retried_attempt_generates_a_new_identity_of_the_same_actor_and_generator`:
   insert, crash before the response, retry, insert: both rows are
   "generated by `client` from `events.id`", they are distinct identities,
   and the response carries the retried attempt's id, whose row the same
   actor inserted).
4. **Generators.** `{"column": "t.c"}` matches the event's `generator`
   index against `Model::generators()`; the column must have a generated
   default (validated). `"call"` matches `CALL_GENERATOR`, so it cannot tell
   two package-call sites apart (no call site is recorded in a `GenId`;
   recording one would change the state encoding and the engine, which this
   track does not own). Discovery uses it only when the actor's code has a
   single site (§3).
5. **Absent values.** `null` and `undefined` are never `same`, never
   generated, and a `match` term that evaluates to either selects no row
   (as SQL `=` does). `returned` of an actor that has not completed, a
   missing `path` field and a missing row are all `null`.
6. **Inserted by.** A row's inserting actor is the actor of its `RowId`
   (the model's logical row identity, kept by UPDATE); rows of the initial
   state match no `insertedBy`. A row inserted by a rolled-back attempt is
   not committed and so is never selected.
7. **Quantifiers** are bounded by the model (actors, committed rows of one
   table) and nest; `self` rebinds in an inner `forEachActor`. A
   quantifier over nothing holds. `returned` uses the latest completed
   attempt: a request that completed twice (a redelivery) told its caller
   twice, and the latest response is the one the client acts on; the
   Phase 13 `actors.returned` count still counts every completion.
8. **Validation** (load time, `Invariant::validate`): unknown tables,
   columns, actors, functions and arguments; `self`, `returned`/`argument`
   without `actor` outside `forEachActor`; `field` outside `forEachRow` or
   naming a column the bound table lacks; `generator.column` without a
   generated default; unknown fields (`deny_unknown_fields`). A `returned`
   read makes the loader record return values and refuse an actor whose
   return expression is not modeled (Phase 13 rule; `returned` inside
   `forEachActor {function: f}` is attributed to `f`, a named actor's
   `returned` to every actor).

## 3. Serial outcomes that are generated values

`SerialCell` (a `HEURISTIC_HYPOTHESIS` of read-modify-write candidates):
"the cell ends with a value some serial execution of the same requests
produces, for the same number of completed requests". `serial_values` runs
the actors one after another in every order and subset (no faults); for
each completed count `k` the cell's values `V_k` are collected, and the
property is `OR_k (completed = k AND OR_{v in V_k} cell ≈ v)`.

Before: a generated `v` refused the whole property. Now `cell ≈ v` is

* `cell = v` for an integer, string or `null` (unchanged);
* `generatedBy(cell, by: {actor: A}, generator: G)` for a generated
  identifier of actor A and generator G **when the renaming is faithful**;
* refused, with a note, otherwise (booleans, rows and clock readings keep
  the Phase 14 note text; an unfaithful identifier says why).

**Faithfulness** (`synth.rs` `renaming`): the serial model is the
concurrent model with the fault bounds set to zero (same plan, same actors
and code), so actor indices and names correspond. A serial event of actor A
and generator G can be stated as "generated by A from G" when A's code (all
its code vectors) has **exactly one generation site** of G (one
`Op::Generate` for the call generator; one INSERT row into the column's
table for a column generator, a multi-row INSERT counting each row) and
**no backward jump**. Then every attempt of A generates at most one value of
G, at that one site, in both models, so "A's value from G" denotes the
same program point in the concurrent run. Event counters are not compared
(they shift with retries and other generators, §2.3). With retries in the
concurrent model, the property accepts any attempt's value of that site
(a weakening, stated in the property's description). Arbitrary strings are
never assigned to generated values.

**Replica progress in serial executions** (goal 3). With the `replicas`
feature, `serial_states` explores every choice of `Replicate` transitions
at every step of a serial execution (depth-first, states deduplicated by
canonical encoding, at most 20 000 states per order or the property is not
instantiated, with the reason). Before, serial executions never replayed
a commit (the replica stayed at the initial state), so a concurrent
execution whose replica *had* caught up (a fresh read) could leave a value
no serial execution produced and be reported against the heuristic
property — an outcome eventual consistency permits. Now stale and fresh
reads are both serial outcomes. Without the feature the computation is the
Phase 14 one, step for step (`run_alone`).

The property's origin is unchanged (`HEURISTIC_HYPOTHESIS`). Its
description gains one clause per new mechanism used ("a generated value is
stated as generated by the same actor and generator ...", "the serial
executions let the modeled asynchronous replica replay any prefix of the
commits at every step ..."), only when used, so earlier descriptions are
unchanged.

## 4. Replica-aware distinctions

* **Stale-but-permitted reads are not bugs by default.** Heuristic
  properties now include every replica progress in their serial outcomes
  (§3), so a stale or a fresh read alone never violates them. Only a
  declared property can require freshness (read-your-writes); the Phase 13-15
  declared properties are unchanged (`display-name-read-your-writes`,
  `nickname-read-your-writes*`, the Phase 13 replica scenario keep their
  verdicts: regression suites).
* **Identity mismatch vs. absence.** When an invariant using Phase 16
  constructs is violated, the witness's final summary carries
  `diagnosis` lines (`Formula::diagnose`; JSON field omitted when empty, so
  earlier witnesses are byte-identical; also printed by the text and HTML
  reports). Each failing identity predicate, per failing quantifier
  instance (`actor \`A\`: ...`, `row {...} of \`t\`: ...`), is classified:
  * `generated-identity mismatch` — every operand is a generated
    identifier, but of another event or of another actor/generator (the
    values are described as `gen#g:a.n (generated by actor \`B\` from the
    default of \`invites.id\`, its generation event n)`);
  * `absent value` — an operand is `null`/`undefined` (a missing row or
    value); with the `replicas` feature the line adds that a read that has
    not yet observed a write returns this, which eventual consistency
    permits unless a declared property requires the write to be visible;
  * `not a generated value of the expected identity` — a concrete value
    where a generated one was expected.
  Comparisons inside quantifiers report their operands' values. Tests
  `a_stale_replica_read_of_a_generated_id_is_an_absent_value` and
  `an_id_generated_by_another_caller_is_an_identity_mismatch`.
* The diagnosis classifies values, not schedules: "absent" does not prove
  the absence came from the replica; the schedule (which shows `replicate`
  steps or their absence) does.
* Replica counterexamples remain model-only (the lockstep oracle has one
  server).

## 5. Original-code and PostgreSQL replay

Which actor generated a value is not observable in an original-code run
(generated texts are matched to events bijectively, not attributed), so a
formula with any Phase 16 construct is classified as **not**
database-only (`reads_only_database*` return false): source verification
reports its invariant as `not-evaluated` and a lockstep-confirmed schedule
uses the wording for invariants that read more than committed rows. This is
conservative: no Phase 16 property is ever reported as violated or holding
on original code.

## 6. Compatibility

* New `Formula` variants (`generatedBy`, `sameGenerated`,
  `distinctGenerated`, `same`, `forEachActor`, `forEachRow`) and `TermOp`
  variants (`countMatching`, `rowMatching`, `returned`, `argument`, `field`)
  are additive in externally tagged enums; new struct fields are
  `skip_serializing_if` defaults; `RowRef` is unchanged. Test
  `earlier_invariants_serialize_byte_identically` fixes the serialized form
  of representative Phase 6-15 invariants and round-trips every invariant
  of every committed scenario file.
* `Formula::eval`/`Term::eval` keep their signatures; old variants
  evaluate exactly as before (an empty binding environment is passed
  through).
* `witness::FinalSummary.diagnosis` is serialized only when non-empty; the
  HTML report emits the same bytes when it is empty.
* `dsl.rs`: `let t = generate "nanoid(21)"` (exactly one string after
  `generate`) is new; anything else after `generate` parses as before.
* Discovery: only `SerialCell` properties whose serial outcomes contain
  generated identifiers, or whose scenario has the `replicas` feature,
  change (§3). Evidence in §8.

## 7. Approximations and limitations

* `generatedBy` matches actor and generator, not the occurrence (§2.3);
  package-call sites are not distinguished (§2.4). Discovery's renaming is
  refused when an actor has several sites of the generator.
* `returned` reads the latest completed attempt only.
* Quantifiers range over the model's actors and committed rows; there is
  no quantifier over generation events or over attempts.
* A diagnosis classifies the final values, not their cause (§4).
* Replica progress in serial executions is bounded (20 000 states per
  order); beyond it the heuristic property is not instantiated (a note
  says so). No serial execution reached the bound in the evaluations below.
* Midday's portal candidate is model-checked with a heuristic property only;
  what callers are told (the `UPDATE ... RETURNING` result) is not modeled
  (Phase 12: returned *values* of an UPDATE are not modeled), so no
  return-value property can be checked on it.

## 8. Evidence

Binaries: "before" is the Phase 15 tree (commit `1efa383`) built in debug
mode; "after" is this track's tree, debug mode. macOS, rustc 1.98.1.

### 8.1 First-party application (`fixtures/phase16/genid`)

`commitrift discover fixtures/phase16/genid --model-check` (with its
`commitrift-contracts.json`):

| entry | candidate | property | status |
| --- | --- | --- | --- |
| `ensureShareToken` (G1) | return-value; read-modify-write | `share-token-told-is-stored` (declared) | MODEL_COUNTEREXAMPLE (both): two first calls each generate a token; the caller whose write lost is told a token that is not stored |
| `ensureShareTokenAtomic` (G1-safe) | return-value | same | NO_VIOLATION_WITHIN_BOUNDS |
| `createProject` (G2) | return-value; resubmitted-write (declared `on-crash` retry) | `returned-project-id-is-the-request-row` (declared) | MODEL_COUNTEREXAMPLE (both): a duplicate submission, or the resubmission after a crash before the response, is told a freshly generated id; the stored row's id was generated by the earlier attempt |
| `createProjectSafe` (G2-safe) | return-value; resubmitted-write | same | NO_VIOLATION_WITHIN_BOUNDS |
| `createInvite` (G3) | return-value | `invite-read-your-writes` (declared) | MODEL_COUNTEREXAMPLE (model only: the replica read precedes the replay of the INSERT) |
| `createInvitePrimary` (G3-safe) | return-value | same | NO_VIOLATION_WITHIN_BOUNDS |
| `recordPageView` (G4) | read-modify-write | `serial-page_views-views` (heuristic) | before: **MODEL_COUNTEREXAMPLE**; after: NO_VIOLATION_WITHIN_BOUNDS |

G4 is the replica false positive §3 removes. The before-binary's
counterexample (with an empty contracts file, since it cannot read the
Phase 16 contracts) was: view 1 reads 10 through the replica and writes
11; the replica replays that commit; view 2 reads 11 and writes 12 — a
*serial* execution with a caught-up replica, final value 12, reported
because the Phase 15 serial outcomes (replica never replayed) were only
{11}. With the declared contracts removed, the after-binary also states
G1's heuristic property (`generatedBy` the actor `ensureShareToken-1` or
`-2` from a package call, per completed count) and it holds; before, it
was refused ("leaves gen#2048:0.0 ... which an invariant cannot state",
INCONCLUSIVE). The heuristic does not see G1's defect; the declared
property about what callers are told does.

### 8.2 Midday (`.corpora/midday`, never executed)

`commitrift discover .corpora/midday --format json --model-check
--no-timing --contracts corpora/contracts/midday.json` (only the replica
factory is declared; nothing about midday's rules):

| candidate | source extraction complete | scenario generated | scenario loaded | property expressible | model check completed | property requires a declaration | unsupported semantics |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `CR-SYNTH-7CC916` read-modify-write of `customers.portalId` (`toggleCustomerPortal`, replica read then primary update writing `nanoid(21)`) | yes (0 unknown operations, none on the path), before and after | yes, before and after (base world; after also 2 further worlds) | before: no (no invariant, nothing to load); after: yes (3 worlds) | before: no ("a serial execution leaves gen#2048:0.0 ... which an invariant cannot state"); after: yes (`generatedBy` actor `toggleCustomerPortal-1`/`-2` from a package call, per completed count; one `nanoid` site per actor) | before: no (INCONCLUSIVE); after: yes, **NO_VIOLATION_WITHIN_BOUNDS** in every explored world (24, 21, 24 states, complete) | no (`HEURISTIC_HYPOTHESIS`) | none on the path; `customers.fts` NOT NULL without a modeled value (a runtime-only note); what callers are told (`UPDATE ... RETURNING`) is not modeled, so no return-value property can be checked |
| `CR-SYNTH-3067C6` write skew on `users_on_team` (`leaveTeam`) | yes (0 unsupported on the entry) | no (BLOCKED before synthesis) | no | no property exists (a multi-row invariant is never inferred) | no | **yes** | none recorded |
| `CR-SYNTH-A06586` write skew on `users_on_team` (`deleteTeamMember`) | yes | no (BLOCKED) | no | no property exists | no | **yes** | none recorded |
| `CR-SYNTH-9FF197` write skew on `insights` (`GenerateInsightsProcessor.process`) | no (89 unsupported constructs on the entry, unsupported on the path) | no (BLOCKED) | no | no property exists | no | **yes** | method calls on values, `date-fns`, `Promise.all` results, `new Date`, ... (first on the path: `generate-team-insights.ts:64:5`) |

Status totals: before 1 INCONCLUSIVE, 3 BLOCKED, model-checkable 0; after
1 NO_VIOLATION_WITHIN_BOUNDS, 3 BLOCKED, model-checkable 1. The portal
result is the verdict of a heuristic hypothesis ("the portal id ends as
some serial execution's"): every final portal id is one a caller generated,
in every explored schedule including replica lag. It is not a safety
claim: the real hazard of this shape — two concurrent first enables each
told a different portal id, one of them dead (fixture G1) — is about the
returned value, which midday's code takes from `UPDATE ... RETURNING` and
the model does not model. No external defect is claimed.

**Templates applied (scratch evaluation, not a declaration, not
committed).** With the replica factory plus all four appendix templates
(`team-keeps-an-owner`, `current-team-is-a-membership`,
`insight-job-reports-a-stored-insight`, `insight-jobs-do-not-fail`) the
team candidates get their properties and scenarios are attempted: both
INCONCLUSIVE, "no valid initial state: actor `leaveTeam-2` /
`deleteTeamMember-2` does not reach ... (ended after
`packages/db/src/queries/teams.ts:35:24` with: User is not a member of
this team)" — the second actor fails the membership check in synthesis.
The insights candidate (and the return-value candidate the declared
formula creates) are UNSUPPORTED at
`generate-team-insights.ts:64:5` ("method calls are not resolved"). So the
templates move the three from "requires a declaration" to the next
blocker; none becomes model-checkable on midday. On the first-party
midday-shaped code the same templates load and are checked (§8.3).

### 8.3 Templates on the first-party application

`every_contract_template_parses_and_loads_against_the_first_party_application`
and a manual run with all four templates pointed at the fixture:
`current-team-is-a-membership` (on `leaveTeam`) NO_VIOLATION_WITHIN_BOUNDS;
`insight-job-reports-a-stored-insight` NO_VIOLATION_WITHIN_BOUNDS;
`insight-jobs-do-not-fail` MODEL_COUNTEREXAMPLE (the second of two jobs
for one period gets no row from `ON CONFLICT DO NOTHING` and throws);
`team-keeps-an-owner` attaches but is not instantiated: the synthesized
membership rows have no `owner` role, so the group violates it in the
initial state ("... violates the invariant in the initial state; not
checked"). Declared for `deleteTeamMember`, `current-team-is-a-membership`
was refused by the loader (`unknown table \`users\``), which is why the
template names `leaveTeam` only.

### 8.4 Compatibility

* Model reports: every committed scenario under `fixtures/` (96 files with
  actors and invariants), `commitrift model-check <file> --no-timing` in
  JSON and text, before vs after: **96 of 96 byte-identical**.
* Discovery (`--model-check --no-timing`, JSON) before vs after on 16
  inputs — `fixtures/phase7/team-app`, `phase8/shop-app`,
  `phase9/ledger-app`, `phase11/commerce-app`, `phase12/eval-app`,
  `phase12/workspace-app`, `phase13/eval-app`, `phase13/replica-app`,
  `phase13/request-app`, `phase14/eval-app`, `phase14/drizzle-app`,
  `phase14/values-app`, `phase14/request-app`, `phase15/eval-app`,
  `phase15/worlds-app`, `.corpora/saas-starter`: **16 of 16
  byte-identical** (the evaluation applications' scores are therefore
  unchanged). Midday: only `CR-SYNTH-7CC916` and the summary/budget
  counters differ (§8.2). The private school-ledger checkout: byte-identical
  (aggregate only).

### 8.5 Tests

`crates/commitrift-model/tests/phase16_genid.rs` (12 tests): identity and
aliasing, unchanged comparisons on generated values, actor/generator
selection, `returned`/`argument`/`forEachActor` binding, term-matched rows
and `insertedBy`, referenced-row identity with `forEachRow`, a retried
attempt's new identity, an id generated before a retry, a stale replica
read (absent value) vs. another caller's id (identity mismatch), empty
diagnoses for earlier invariants, validation errors, round-trip of every
new construct, the fixed serialization of earlier invariants and a
round-trip of every invariant in every committed scenario, and the DSL's
`generate`. `crates/commitrift-discover/tests/phase16_genid.rs` (7 tests):
§8.1 and §8.3. Commands and counts are in the track report.

## Appendix: contract templates

CommitRift never infers a multi-row invariant, so midday's three
write-skew candidates stay BLOCKED until a developer declares one. These
are **templates, not findings**: each states a business rule that midday's
code does not state, and is correct only if it is the developer's rule.
Copy the chosen objects into the `invariants` array of a
`commitrift-contracts.json` (`docs/user-guide/contracts.md`), adjust names,
values and entries, and rerun `commitrift discover --model-check
--contracts FILE`. The selectors are midday's (commit `51587319`); the
templates use only the real declaration syntax. Test
`every_contract_template_parses_and_loads_against_the_first_party_application`
(`crates/commitrift-discover/tests/phase16_genid.rs`) parses every block
below as a contracts file, points its entries at the midday-shaped code of
`fixtures/phase16/genid` (same table and column names), runs discovery and
checks that the property attaches to a candidate and is model-checked.

### Team membership (`leaveTeam`, `deleteTeamMember`: write skew on `users_on_team`)

A team keeps at least one owner. Choose it if removing the last owner
must be impossible; a concurrent `leaveTeam` of two owners is then the
write skew the candidate describes.

```json
{
  "name": "team-keeps-an-owner",
  "description": "Every team that has members keeps at least one member with the owner role.",
  "entries": [
    "packages/db/src/queries/teams.ts#leaveTeam",
    "packages/db/src/queries/teams.ts#deleteTeamMember"
  ],
  "property": {
    "template": "atLeast",
    "table": "users_on_team",
    "groupBy": ["teamId"],
    "where": { "role": "owner" },
    "min": 1
  }
}
```

(`atLeast` is instantiated per group of the synthesized initial rows; a
group that violates it in the initial state is not checked, with a note.)

A user's current team is one they belong to (Phase 16 `forEachRow` and
`countMatching`). Choose it if `users.teamId` must always name a
`users_on_team` membership of that user. It is declared for `leaveTeam`
only: a synthesized scenario contains the tables its actors use, and
`deleteTeamMember` never touches `users`, so for it the loader refuses the
formula (`unknown table \`users\``, verdict `NOT_LOADED`, measured on the
fixture). Note that `deleteTeamMember` leaves `users.teamId` unchanged, so
under this rule it is a defect even without concurrency.

```json
{
  "name": "current-team-is-a-membership",
  "description": "A user's current team (users.teamId) is null or a team the user is a member of.",
  "entries": [
    "packages/db/src/queries/teams.ts#leaveTeam"
  ],
  "property": {
    "template": "formula",
    "assert": {
      "forEachRow": {
        "table": "users",
        "assert": {
          "or": [
            { "eq": [{ "field": "teamId" }, null] },
            {
              "ge": [
                {
                  "countMatching": {
                    "table": "users_on_team",
                    "match": { "userId": { "field": "id" }, "teamId": { "field": "teamId" } }
                  }
                },
                1
              ]
            }
          ]
        }
      }
    }
  }
}
```

### Insight generation (`GenerateInsightsProcessor.process`: write skew on `insights`)

Every completed job reports an existing insight (Phase 16 `forEachActor`,
`returned`, `countMatching`; the insight's id is database-generated).
Choose it if a job's `insightId` must name the period's stored insight.

```json
{
  "name": "insight-job-reports-a-stored-insight",
  "description": "Every completed insight job that reports an insightId reports the generated id of a stored insight.",
  "entries": [
    "apps/worker/src/processors/insights/generate-team-insights.ts#GenerateInsightsProcessor/method[process]"
  ],
  "property": {
    "template": "formula",
    "assert": {
      "forEachActor": {
        "assert": {
          "implies": [
            { "ne": [{ "returned": { "path": "insightId" } }, null] },
            {
              "eq": [
                { "countMatching": { "table": "insights", "match": { "id": { "returned": { "path": "insightId" } } } } },
                1
              ]
            }
          ]
        }
      }
    }
  }
}
```

A duplicate job does not fail. Choose it if two jobs for one period may
overlap and neither should end in "Failed to create insight record" (the
`ON CONFLICT DO NOTHING` of the second job's insert returns no row).

```json
{
  "name": "insight-jobs-do-not-fail",
  "description": "No insight job fails.",
  "entries": [
    "apps/worker/src/processors/insights/generate-team-insights.ts#GenerateInsightsProcessor/method[process]"
  ],
  "property": {
    "template": "formula",
    "assert": { "eq": [{ "actors": { "outcome": "failed" } }, 0] }
  }
}
```

"At most one insight per team and period" needs no declaration: the
`insights_team_period_unique` constraint enforces it (a `SCHEMA_ENFORCED`
property).
