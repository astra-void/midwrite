# Precision changes (Milestone 2)

Every change below is intentional, is covered by tests, and is listed with
the Phase 1 behavior it replaces. Diagnostic codes keep their meaning.

## 1. Branch condition correlation

**Phase 1.** All syntactic paths were assumed feasible; `if (flag) write;
if (!flag) publish;` was reported as CP1001 (medium).

**Phase 2.** The frontend records, for each branch arm, the facts taking it
implies about *variables* (`ir::Fact`, on `ir::Arm::facts`). Path enumeration
discards a trace whose new decision contradicts an earlier one on the same
trace. This is not a solver: facts are only

| Condition | True arm | False arm |
| --- | --- | --- |
| `v` | `v` truthy | `v` falsy |
| `!e` | facts of `e`, negated | |
| `a && b` | facts of `a` and of `b` | nothing |
| `a \|\| b` | nothing | `a` falsy and `b` falsy |
| `v === lit` / `v !== lit` | `v === lit` / `v !== lit` | the opposite |
| `v == null`, `v == undefined` | `v` falsy | nothing |
| `v == true` | `v` truthy | nothing |
| `switch (v)` | `case lit:` → `v === lit`; `default` / no case → `v !== lit` for every literal case | |
| `a ?? b` | right operand runs → `a` falsy | nothing |
| literal or `const x = literal` | arm impossible when the value contradicts it | |

`lit` is a boolean, string, finite number, `null` or the global `undefined`.
A *variable* is an identifier that resolves to a binding of the file that is
never written after its declaration (`symbol_is_mutated` in Oxc) and is not
an import (imports are live bindings). Member expressions (getters),
calls (two calls may return different values), loose comparisons with
strings, relational operators and everything else imply nothing and never
discard a path. A reassigned variable is never a variable, so an assignment
between two tests invalidates the knowledge.

Facts of an inlined callee are renamed per call (`call#3:flag#12`), so they
never constrain the caller or another call of the same callee.

Tests: `crates/crashproof-core/src/analysis/tests.rs`
(`contradictory_branches_are_eliminated`, `compatible_branches_are_preserved`,
`conditions_without_facts_stay_feasible`,
`facts_on_different_variables_do_not_interact`,
`literal_equalities_are_mutually_exclusive`, `never_arms_are_not_taken`,
`switch_cases_imply_equalities`) and
`crates/crashproof-analyzer/tests/phase2.rs`
(`contradictory_boolean_branches_are_eliminated`,
`compatible_branches_are_preserved`, `reassignment_invalidates_knowledge`,
`members_calls_and_complex_conditions_stay_unknown`,
`strict_equalities_with_different_literals_are_exclusive`,
`switch_cases_and_ternaries_correlate_with_later_tests`,
`constant_conditions_remove_arms`), and the project fixture
`fixtures/projects/correlated`. The existing fixture
`fixtures/safe/mutually_exclusive.ts` and all vulnerable fixtures are
unchanged.

Measured effect: 112 paths pruned in midday, 759 in school-ledger. The
pruning did not remove any Phase 1 diagnostic in the corpus (no Phase 1
finding there depended on contradictory simple conditions); its effect is in
the fixtures above and in keeping inlined callees from producing infeasible
combinations.

## 2. Confidence semantics

**Phase 1.** `confidence` described the path only; a `this.db` member-path
match (only names checked) could be `high`. The Phase 1 decision "basis never
changes confidence" is reversed here, as the Phase 2 brief asked.

**Phase 2.** Two dimensions are reported separately (additive JSON fields):

* `pathCertainty`: `straightLine`, `conditional`, `approximated`,
  `truncated`, `unknown`;
* `evidenceStrength`: the weakest recognition among the operations the
  finding relies on (`provenance`, `configuredImport`, `configuredName`,
  `memberPath`, `unknown`).

`confidence` is the minimum of what the path allows (unchanged rules) and
what the evidence allows: `memberPath` caps it at `medium`. Configured names
are not capped: they are explicit assertions by the user, and capping them
would have made almost every finding `medium` without new information.
`kind` (potential hazard or analysis limitation) and `reproduced` (always
`false`) are unchanged.

Fixture changes: five findings that rely on member paths went from `high` to
`medium` (`fixtures/vulnerable/member_receivers.ts` ×4, the CP1002 of
`fixtures/vulnerable/charge_then_write.ts`, whose `stripe.charges` is a member
path). Test: `member_path_evidence_caps_confidence`.

**Unawaited transactions.** A finding that depends on the commit of a
transaction that is not awaited (`db.transaction(...)` as a statement) was
`high` in Phase 1 although the commit is unordered with later code. It is
now at most `medium` (`conditional`), with a note. Test:
`transaction_model_audit` (case "an unawaited transaction is unordered").

## 3. CP1004 and CP1005 policy

**Evidence.** In the Phase 1 baseline, 96 of 123 warnings on the corpus were
CP1004 and 20 were CP1005. The Phase 2 review
(`docs/phase2/CORPUS-EVALUATION.md`) found:

| Code | Reviewed (decided) | Real window | Likely bug |
| --- | --- | --- | --- |
| CP1004 | 43 | 42 | 12 |
| CP1005 | 8 | 7 | 0 |
| CP1001 + CP1002 | 46 | 43 | 15 |

and that, with tests and seed scripts scanned, CP1004 volume is dominated by
test setup (all 123 CP1004 of school-ledger). The data does **not** show that
CP1004 is less precise than CP1001 in application code (bug precision is low
for both); it shows that CP1004 and CP1005 are far more numerous, and that
two sequential effects on the same resource are usually ordered on purpose,
while a database/external pair cannot be made atomic by any transaction.

**Change.** A small policy switch (`--policy`, `AnalysisOptions::policy`):

| Policy | CP1001–CP1003 | CP1004 | CP1005 | CP9001 |
| --- | --- | --- | --- | --- |
| `recommended` (default) | warning | warning only for a write that bypasses its enclosing transaction or uses a handle after its transaction ended; otherwise info | info | info, unrecognized calls grouped per function |
| `strict` | warning | warning | warning | info, one per call (Phase 1) |
| `audit` | warning | warning | warning | as strict, plus unknown calls before the first recognized operation |

The codes, messages and the meaning of each code are unchanged; only
severity, and so the exit status at `--fail-on warning`, differs. The
bypass case stays a warning because it is a specific mistake (the write
should use `tx`), not a judgment about whether two writes belong together.

**Cost, measured.** Under `recommended`, 12 reviewed true positives (all
CP1004) are reported as `info` instead of `warning`; they still appear in
every output format. Teams that want them to fail CI use `--policy strict`.

**Default change.** `crashproof check` without `--policy` now exits 0 for a
file whose only finding is a non-bypass CP1004 (Phase 1: 1). The CLI test
`findings_take_precedence_over_parse_failures_in_the_exit_code` was updated
to cover both policies.

## 4. Transaction correctness audit

Checked end to end (`crates/crashproof-analyzer/tests/phase2.rs`,
`transaction_model_audit`, plus the existing fixtures):

| Case | Result |
| --- | --- |
| Writes on the handle inside a callback | atomic, no finding |
| Write on the root `db` inside a callback | CP1004, a warning under every policy |
| External effect inside a callback with writes | CP1003; a transaction never makes an external effect atomic |
| Write awaited inside a callback that then throws | rolled back, never treated as durable |
| Early `return` in a callback | commits |
| Savepoint rolled back | only its own writes are discarded |
| Non-inline callback | CP9001 (opaque transaction) |
| Unawaited transaction | modeled, confidence at most `medium` (changed, see above) |
| Helper receiving `tx` (same file or imported) | its writes join the transaction (was a false CP1004 bypass in Phase 1) |
| Helper receiving `db` inside a transaction | CP1004 bypass, warning |
| Helper receiving an unrecognized value | its writes are unknown; no bypass is claimed |

The helper cases were the one high-severity correctness bug found: Phase 1
analyzed a helper's parameter named like a configured receiver (`db`) as the
root handle even when the caller passed `tx`, and reported a `high`-confidence
CP1004 "bypass". It occurred in the corpus (midday
`packages/db/src/queries/teams.ts:280`, two diagnostics). Tests:
`a_transaction_handle_argument_joins_the_callers_transaction`,
`a_root_handle_argument_bypasses_the_callers_transaction`,
`an_unrecognized_argument_is_never_guessed_to_be_a_handle`,
`a_helper_receiving_the_transaction_handle_joins_the_transaction`, and the
project fixtures `tx_handle`, `root_handle`, `unknown_handle`.

## 5. Noise reductions that keep the information

* **Alternative partners merged.** A write or effect preceded by different
  commits on different paths was reported once per predecessor (four CP1004
  at saas-starter `app/(login)/actions.ts:210`). Pairs sharing their primary
  operation (same reported location and label) are now one diagnostic, for
  the best-ranked path, listing the alternatives in `limitations`. In the
  corpus this merged 17 Phase 1 CP1004. Test:
  `alternative_predecessors_are_reported_once`.
* **Unknown calls inside an inlined call** are reported once per call, with
  the others listed (cross-module summaries otherwise repeat every unknown
  call of a callee in every caller).
* **Context-only unknown calls** (unrecognized calls reported only because
  something committed before them) are one CP9001 per function under the
  default policy, listing the others; `strict` keeps one per call. Unknown
  operations that certainly involve the database (raw SQL, opaque
  transactions, unsupported receivers) are still reported one by one.

## 6. Fixture contracts

Single-file fixtures: the same 51 `(code, line)` findings as Phase 1 (two
fixtures gained two comment lines, which moved their expectations down by
two lines), with five confidence changes (section 2). Twenty multi-file
fixture projects were added under `fixtures/projects/`
(`crates/crashproof-analyzer/tests/projects.rs`).
