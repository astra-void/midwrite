# Phase 12 security and execution boundaries

* **Static analysis never executes code.** `$defaultFn` callbacks are
  recognized from their syntax and their imports, never called; package
  functions are never assumed pure; a database factory is recognized from its
  declared return type, not by running it.
* **Application code runs only** (a) through the existing, separately
  authorized `commitrift model-check --verify-source --allow-execution` for
  the first-party `fixtures/phase12/workspace-app` (and, as before, the
  Phase 7–11 first-party fixtures), and (b) in the first-party Drizzle probe
  runner (`fixtures/phase12/defaults-probe/verify/probe.js`) started by a
  test against a disposable PostgreSQL cluster in a private directory. Both
  use only local Unix sockets.
* **No corpus project was executed.** saas-starter and midday (public,
  pinned) and the two private projects (school-ledger at `4b97f34`,
  unnamed-ai-project at `39fcfc7`, read from `git archive`s of those
  revisions in a private scratch directory; the working trees were not
  touched) were analyzed statically. For the private projects only aggregate
  counts and generic reason categories are written; no code, path or
  identifier of theirs appears in reports or documentation.
* **Generated values in reports** are rendered as `gen#g:a.n` or
  `{"$generated": …}`; runtime UUIDs appear only in original-code
  verification reports of first-party fixtures.
* **Network**: the tests run without network. The Linux evidence container
  (`scripts/phase12-linux-docker.sh`) reuses the Phase 4–10 image and runs
  `npm ci` for the fixtures (registry download), as in earlier phases.
* **Escaping**: discovery and model-check HTML reports escape every string
  (existing tests `the_html_report_escapes…`, Phase 11's discovery HTML test);
  the new candidate class and generated-value renderings are plain text
  through the same escaping.
* The disposable clusters' socket directories are short paths under the
  system temporary directory (Unix socket path limits), created private by
  the existing `ReplayRoot`; the probe cluster used during development was
  stopped and lived in the session scratch directory.
