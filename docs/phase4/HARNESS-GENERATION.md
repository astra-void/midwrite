# Trace-to-harness generation (`commitrift init-replay`)

Phase 4 milestone 4. Evidence: `reports/phase4/harness-generation/`
(`trace.json`, the raw generated files in `raw/`, `manual-changes.diff`),
the completed harness `fixtures/replay/drizzle-node/generated-harness/`.

## Command

```bash
commitrift trace src --format json > trace.json        # in the project root
commitrift init-replay --trace trace.json --finding <FINGERPRINT> \
  --runtime node --output commitrift-replay            # a new directory inside the project
```

| Option | Meaning |
| --- | --- |
| `--trace` | A trace report. Only `kind: commitrift.traces` is accepted. |
| `--finding` | Fingerprint or trace id. A finding without a trace is refused with its reason; an `incomplete` trace is refused unless `--allow-incomplete`. |
| `--runtime` | `node` (default; compiled with `tsc`) or `bun`. |
| `--output` | Must be a new or existing subdirectory **inside** `--project`. |
| `--project` | The directory `commitrift trace` ran in (default `.`). The finding's file must exist under it, else "pass the directory `commitrift trace` ran in". |
| `--analyze`, `--config` | What the generated manifest analyzes (default: the top directory of the finding's file; the project's `commitrift.json`). |
| `--force` | Overwrite exactly the generated files; other files are never touched. |

## Safety properties

* **Executes nothing, installs nothing, contacts nothing.** It reads the
  trace report and (to verify the link, below) the project sources.
* **Writes only into `--output`**, which must lie inside the project and
  cannot be the project root. Existing files are refused without `--force`
  (`create_new`, so an existing symbolic link is never followed); every
  target directory is resolved first and must stay inside `--output` (a
  pre-existing `commitrift-runtime -> /elsewhere` link is refused before
  anything is written). Tests:
  `init_replay_writes_a_skeleton_linked_to_the_finding_that_refuses_to_run`,
  `init_replay_refuses_bad_requests`,
  `init_replay_never_writes_through_a_symlinked_subdirectory`.
* **Text from the trace cannot become code.** Labels, names and paths
  interpolated into `//` comments of `harness.ts` and `--` comments of
  `schema.sql` have control characters (newlines) replaced, so a tampered
  trace file cannot inject TypeScript or SQL (`interpolated_text_cannot_leave_a_comment`).
* **Fails clearly until completed.** Everything CommitRift cannot know is a
  `COMMITRIFT-TODO` placeholder. `commitrift replay` refuses a manifest that
  contains the marker anywhere (keys or values; the error lists each JSON
  pointer), a setup SQL file that contains it, and a compiled entry point
  that contains it. The scenario stub also throws.

## What is generated, and from what

| File | Content | Derived from |
| --- | --- | --- |
| `commitrift-replay.json` | `runtime`, `command` (`node dist/<dir>/harness.js`), disposable database, `analysis` (`root` = path back to the project, so fingerprints match), placeholder observations (one per effect at the boundary), one placeholder invariant, control run, cases | the trace |
| cases | `kill-<boundary>` and `exception-<boundary>` at the trace's boundary checkpoint (with label = table where known) plus a `verifyBeforeInjection` placeholder; `kill-before-<second effect>` (the window's other end); `negative-kill-before-<first effect>` and `negative-kill-after-<second effect>`; every case linked by **exact fingerprint**; **no `expect`** | injection boundary, checkpoint hints, step kinds |
| `harness.ts` | wiring of the instrumented `pg` pool and the event-log transport, a scenario stub named after the function, the source span of every database write and external effect on the trace, a suggested import path | trace steps and locations |
| `schema.sql` | placeholder, plus the tables the trace touches | trace tables |
| `package.json`, `tsconfig.json` | `@commitrift/replay` from the vendored copy; `pg` and `typescript` resolve from the project; `tsconfig` compiles the harness and the analyzed sources | — |
| `README.md` | provenance, the trace text, a checkpoint table with source spans, the manual steps, how to run | the trace |
| `finding.json` | the trace itself (provenance) | — |
| `commitrift-runtime/` | the replay client and adapters, byte-identical to `runtime/node` of this binary | — |

Not generated, on purpose: SQL queries, topics, inputs, business invariants,
expected outcomes, queue implementations. A guessed query that happens to
run would look like evidence.

**Checkpoint placement needs no source change.** The adapters emit the
protocol checkpoints around real driver calls, so the generator suggests
*where* the boundary is (source spans in the README and in `harness.ts`)
but never patches the application. No source-patch mode was implemented.

**Identity check.** Before writing, the generator re-runs the static
analysis exactly as the generated manifest will (same root, paths and
configuration, read-only) and warns if that does not reproduce the
finding — e.g. a trace produced from another directory.

## Demonstration on the real Drizzle fixture (2026-10-25)

1. `commitrift trace src --format json` in `fixtures/replay/drizzle-node`
   → finding `4f53a6215ac16913` (CP1001 in `completeOrder`).
2. `commitrift init-replay --trace … --finding 4f53a6215ac16913 --output generated-harness`
   → 7 files + vendored client; `commitrift replay generated-harness`
   refuses: *13 unfinished placeholder(s) at /cases/0/input/COMMITRIFT-TODO, …*
3. Manual integration (`manual-changes.diff`, 66 changed lines): `usePool(pool)`,
   `useTransport(publishToEventLog)`, the dynamic import and call of
   `completeOrder`; `CREATE TABLE orders` + one row; the two observation
   queries (`SELECT status …`, topic `order.completed`); the invariant
   (paid ⇒ ≥ 1 event); inputs; `verifyBeforeInjection` = `paid`; after the
   first run, `expect` values. (The raw copy was generated under a
   temporary directory name, which is why the entry path also differs.)
4. `npm run build` (tsc), then `commitrift replay generated-harness --allow-execution`:

| Case | Result |
| --- | --- |
| `kill-after-db-commit` | RUNTIME_REPRODUCED, linked to `4f53a6215ac16913`, `onTraceBoundary` |
| `exception-after-db-commit` | RUNTIME_REPRODUCED |
| `kill-before-event-publish` | RUNTIME_REPRODUCED |
| `negative-kill-before-db-write` | RUNTIME_NOT_REPRODUCED |
| `negative-kill-after-event-publish` | RUNTIME_NOT_REPRODUCED |

Covered by `the_completed_generated_harness_reproduces_the_drizzle_dual_write`
(also checks the vendored client has not drifted), run on macOS and Linux.

## What still requires manual work

* Exposing (or already having) a seam to hand the application a pool and a
  transport. Applications that construct their database client at import
  time from environment variables need a small refactor or a test entry
  point.
* Writing the observations and the invariant: the core of the evidence,
  deliberately left to the developer.
* Compiling the application's sources: the generated `tsconfig.json` only
  suits projects that compile with plain `NodeNext` settings; projects with
  path aliases or a bundler should import their own build output instead.
* Drivers other than node-postgres (postgres.js, Neon, Bun SQL) need an
  adapter equivalent to `pg.js`; external effects other than an event-log
  publish need an adapter that emits `before/after-event-publish`.
