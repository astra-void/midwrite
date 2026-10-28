# Phase 15 security and privacy

The execution safety model is unchanged.

* **Discovery stays execution-free.** World search runs the model's engine
  on extracted code (as Phase 11 synthesis did); no application code, no
  package code and no database is touched by `discover`. The report keeps
  `executesApplicationCode: false`.
* **Annotation reading** (literal-union aliases) parses project files only,
  follows relative imports only (never packages or absolute paths), and is
  depth-limited (4).
* **Original-code verification** still requires `--verify-source
  --allow-execution`, a harness manifest the user wrote
  (`commitrift-source.json`), and runs on disposable PostgreSQL clusters
  created and removed by CommitRift. In this cycle it ran only on the
  first-party fixture `fixtures/phase15/worlds-app` (and the existing
  first-party fixtures in the test suite). No third-party or private
  application was executed; school-ledger, midday and saas-starter were
  analyzed statically only.
* **No production databases, no environment secrets.** Scenarios,
  reproduction artifacts and synthesized preconditions contain only
  synthesized values (`name-1`, stable UUID literals, enum members, string
  literals copied from the source's own comparisons). Reproduction
  artifacts carry FNV-64 fingerprints of source files, not their contents.
* **Synthesized authorization preconditions** choose callers for which the
  application's own guards pass; they never remove a guard, never mark a
  caller as authorized outside the scenario, and are stated in every
  scenario that uses them.
* **Private corpus**: only aggregate counts and generic causes of
  school-ledger appear in reports; no selector, path or identifier of it
  is written to `reports/` or `docs/`.
* **Linux**: the container run mounts the repository read-only and writes
  only its output directory; the image build and `npm ci` download
  packages (as in Phases 12–14); the tests themselves run without network.
* Nothing was committed, pushed or published.
