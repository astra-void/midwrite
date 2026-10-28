# Phase 13 security

Unchanged guarantees, and what Phase 13 adds to them.

* **Static discovery never executes application code.** The new analyzer
  work (handle provenance through `Awaited<ReturnType<typeof f>>`, declared
  replica factories, function aliases, package contracts) reads source and
  declared contracts only; a declared replica factory's body is not
  interpreted, and a package contract never runs the package.
* **Runtime verification requires `--allow-execution`** and an authorized
  harness manifest. Phase 13 ran original code only for the first-party
  fixtures `fixtures/phase13/request-app` and `fixtures/phase12/workspace-app`,
  on private disposable PostgreSQL clusters (unix socket under a short
  temporary directory, `listen_addresses=''`), deleted afterwards. No
  external, corpus or private application was executed. Midday,
  school-ledger and unnamed-ai-project were analyzed statically, the two
  private ones from read-only copies (no `node_modules`), without modifying
  their working trees.
* **The response checkpoint** adds one message type (`respond`) to the
  worker protocol; it is active only when the coordinator sets
  `COMMITRIFT_SOURCE_RESPONSE=hold`, and only after the harness entry
  returned. The coordinator plays the client; no network listener is opened.
* **The PostgreSQL ground truth** (`scripts/phase13-lock-matrix.py`) starts
  its own disposable cluster from the PostgreSQL 17 binaries and connects to
  nothing else.
* **Read replicas** are a model abstraction: no replication was configured
  or connected to; the lockstep oracle and original-code verification refuse
  replica schedules.
* **No production database** was contacted. No package was installed into a
  corpus project. The request-app fixture reuses the pinned packages of the
  Phase 12 workspace fixture (`node_modules` is a local symlink; its
  `package-lock.json` pins the same versions for `npm ci` on Linux).
* **Privacy**: reports about school-ledger and unnamed-ai-project contain
  aggregate counts and generic categories only (checked: no paths,
  selectors or identifiers in `reports/phase13/analyzer/corpus/*` and
  `school-ledger-candidates.json`). Committed reports contain no SQL
  parameters from private projects.
* **No commits, pushes or publications** were made. Report HTML escaping
  and CSP are unchanged (Phase 6/11 tests pass).
