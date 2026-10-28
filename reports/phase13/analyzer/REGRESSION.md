# Phase 13 analyzer regression check (D9)

Old binary: the Phase 12 commit (`32efbca`) built from a clean `git archive
HEAD` copy; new binary: this branch's analyzer changes (plus the shared
Phase 13 interface patch). Same machine, same inputs; private projects read
from read-only copies. Baselines were not updated.

## Static `commitrift check` / `trace`

`check-identity.txt` (`scripts/phase8-identity.sh` with the private projects
pointed at the copies, old binary passed by absolute path): saas-starter,
midday, school-ledger, unnamed-ai-project × depths 3, 6, 12 × path modes
shared, compact, legacy × `check` json/sarif/text and `trace` json:
**144/144 byte-identical**. The static checker does not use the Phase 13
extractor code.

## `commitrift discover` (where the changes are)

`corpus/SUMMARY.md`, and a per-entry comparison of full reports
(`--no-timing`):

| project | difference | classification |
| --- | --- | --- |
| saas-starter | same entries, statuses, candidates and verdicts; the UNSUPPORTED `signUp` candidate's finding now names the bcryptjs contract; 6 entries' unknown details name the contract (12 calls) | intended improvement (precision) |
| unnamed-ai-project | same counts and statuses; 16 entries' unknown details changed (62 package calls now name a contract) | intended improvement (precision) |
| midday, no declaration | 2 read-only entries → no database effects: Phase 12 bound their 3 SELECTs through the replicated `db` to the primary by the receiver name; now "database handle of an unresolved type". 409 entries' unknown details changed (unbound handle reasons) | intended improvement (removes a misbinding) |
| midday, declared factory | 47 entries → effectful, 62 → read-only, 3 new candidates; 86 replica reads; new categories (column not modeled 6 → 179, unsupported query shape 3 → 112, unsupported transaction shape 434 → 532) | intended improvement; the new categories are newly exposed behavior |
| school-ledger | 7 candidates UNSUPPORTED → INCONCLUSIVE (alias resolved; synthesis then finds the guarded path unsatisfiable); 2 findings re-worded; 1 entry no-database-effects → read-only; unknown operations 12 894 → 14 615 | intended improvement; the rise is newly exposed behavior (aliases and React `cache` functions inlined) |
| midday, school-ledger | wall +0.17 s / +0.10 s, peak RSS +2 MiB / +11 MiB | measured cost (project-wide `withReplicas` scan; more inlining) |

No difference was classified as a regression, and none as inconclusive.

## Tests

`cargo test -p commitrift-analyzer -p commitrift-discover`: 214 passed, 0
failed, 1 ignored (`test.txt`); `cargo clippy -p commitrift-analyzer -p
commitrift-discover --all-targets --features commitrift-model/postgres -- -D
warnings` clean (`clippy.txt`; without `--features commitrift-model/postgres`
the model crate's `RUNTIME_NOTE` is dead code in that feature set, unrelated
to this change); `cargo fmt --all -- --check` clean (`fmt.txt`).

CLI tests `commitrift-cli --test phase11 --test phase12` in this worktree:
the discovery tests pass; `generated_scenarios_never_run_application_code_without_authorization`
and the three Phase 12 PostgreSQL/Drizzle tests fail because the worktree's
fixtures have no `node_modules`/`dist` (`npm run build` cannot run here) —
an environment limitation, not caused by these changes.
