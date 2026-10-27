# Phase 11 limitations

Everything in `docs/phase10/LIMITATIONS.md` still applies.

## Evidence

* **No independent human review.** No finding was reviewed by anyone
  outside this work; AI evaluation is not review.
* **The first-party benchmark is not independent**: fixture, labels and
  engine have the same author, and the fixture was written with the
  detector families in mind. Its scores are an upper bound, not an
  estimate.
* **The held-out set stopped being held out** after its first scoring: the
  engine was changed because of what it showed. Both scores are reported
  (`PRECISION-RECALL.md`); only the first is a held-out result.
* **No external application produced a verified finding.** On the corpus,
  one model counterexample (saas-starter), against a heuristic hypothesis,
  never executed. Three of four corpus projects yield no candidate because
  their tables are refused (generated primary keys).
* Runtime reproduction is evidence about one schedule, one input, one
  driver version and one PostgreSQL (17.11 on macOS); Linux was not run for
  Phase 11.

## Discovery

* **Properties are hypotheses unless declared or schema-enforced.**
  `SOURCE_ASSERTED` is defined but no rule produces it. Recovery
  requirements (an event is not lost, a payment is eventually recorded) are
  never synthesized.
* Eight detector families; each is a syntactic-semantic rule over the
  extracted IR, not a general dependence analysis. Write skew needs a
  declared invariant; duplicate processing and external-effect gaps need a
  declared delivery or retry, except retries the code itself performs.
* Concurrency of exported functions that are not request handlers is a
  `discovery-default` assumption.
* Pairing covers check-then-act and read-modify-write only; a resource with
  more than six writers is truncated (counted).
* Feasibility filtering is limited to control flow and literal filters;
  tenant separation through values the model cannot compare statically is
  left to the model on a synthesized state (which uses equal keys by
  default and a distinct-key control).
* Entry discovery skips `export default`, export lists and re-exports.

## State synthesis

* Only closed predicates made of comparisons with literals are satisfied;
  `OR`, `IN`, `LIKE`, functions and expressions over two columns are not.
* Joins: missing base rows and first-level joined rows only.
* Arguments are typed by the columns they meet; values that validation code
  (not modeled) would reject must be declared (`values`).
* The argument/row correlation ("`member` is the `members` row") is a
  heuristic stated as a note, chosen by foreign-key evidence when several
  tables fit.
* Count reads are satisfied by adding rows until the guard passes (≤ 16
  iterations); a guard that needs fewer rows is not reached.
* Triggers, row-level security, CHECK constraints the extractor does not
  read, and partial unique indexes are not modeled; a synthesized state is
  valid only with respect to the modeled constraints and foreign keys.
* A generated scenario reads one schema module; tables from several are
  refused.

## Extraction (inherited)

* The Phase 7 extractor emits a helper's return slot with the caller's span
  resolved in the callee's file, so that local operation's source line is
  wrong. Phase 11 only prevented the panic it could cause; correcting the
  location would change existing model-check reports.
* A generated scenario whose local is assigned only by an unsupported
  operation does not load (saas-starter `signUp`); the candidate is
  `INCONCLUSIVE`.
* A helper returning a mutation's rows (`return db.update(...).returning()`)
  is unsupported ("an array of rows used as a value").
* Extraction re-indexes the project per entry; on large projects discovery
  is dominated by it (`PERFORMANCE.md`).

## Runtime evaluation of outcome terms

Outcome terms (`actors`) are evaluated after an original-code run only when
the plan has no crash, loss, restart or delayed completion, and every
observed outcome equals the model's.
