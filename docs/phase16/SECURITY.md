# Phase 16 security and privacy

The execution safety model is unchanged. User-facing instructions:
[docs/reference/security.md](../reference/security.md).

* **Discovery stays execution-free.** None of the Phase 16 additions runs
  application or package code:
  * zod validation is evaluated by the model's own `zodfn` from the schema
    read out of the source;
  * loops are unrolled in the IR;
  * `request.json()` is the entry's argument, a synthesized value;
  * Drizzle predicates are resolved statically.
  
  The report keeps `executesApplicationCode: false`. The Phase 16 diff adds
  no process spawning outside tests (checked: no new `Command::new` or
  `std::process` use in `crates/*/src`).
* **One new file read: the zod version.** To select zod 3 or zod 4
  semantics, the analyzer reads `node_modules/zod/package.json`, or else
  the nearest `package.json` that declares zod (following `catalog:`
  references). It reads metadata only, never package code. Only
  directories from the importing file up to the project root are read,
  never above it. When no version is found, the version is "unknown" and
  every version-dependent behavior is refused.
* **Original-code verification** still requires all of:
  * `--verify-source --allow-execution`;
  * a harness manifest the user wrote (`commitrift-source.json`);
  * disposable PostgreSQL clusters that CommitRift creates and removes.
  
  In this cycle it ran on first-party fixtures only
  (`fixtures/phase16/retry`, `fixtures/phase15/worlds-app` and the existing
  fixtures of the test suite). The connection-loss reproductions use the
  existing local TCP proxy between the harness and the disposable cluster;
  it never connects anywhere else.
* **Test-only execution of third-party code.** The zod differential test
  runs Node.js on the pinned zod packages (`fixtures/phase16/zod/run.mjs`)
  to compare them with the model. This is test code with pinned versions,
  never part of `discover` or `model-check`.
* **No external application was executed or modified.** saas-starter,
  midday, school-ledger and unnamed-ai-project were analyzed statically
  from their pinned snapshots. No file in any of the four checkouts was
  modified during Phase 16. The public snapshots are clean. The two
  private checkouts carry local changes of their owner dated before this
  phase, which were left untouched.
* **Private corpus.** Only aggregate counts and generic causes of
  school-ledger and unnamed-ai-project appear in reports and documents: no
  selector, path, identifier or source text.
  * `scripts/phase16-corpus.py` keeps candidate lists, with blocker text,
    for public projects only. For private ones it writes only the generic
    blocker category.
  * The intermediate full reports used for the stage analysis were written
    to the session's temporary scratch directory, outside the repository.
* **No production databases, no secrets.** Scenarios and reproduction
  artifacts contain only synthesized values. Synthesized zod-conforming
  inputs are built from the schema's own checks (enum members, lengths,
  UUID and email shapes), never copied from data.
* **Closed Alpha package.** `docs/alpha/` is prepared but **not
  distributed or published**. Nothing is pushed to a registry or sent to
  anyone. Its bug-report and feedback templates ask testers not to include
  secrets, credentials or customer data.
