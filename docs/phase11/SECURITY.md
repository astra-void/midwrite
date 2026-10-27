# Security and privacy (Phase 11)

The Phase 10 model (`docs/phase10/SECURITY.md`) is unchanged.

* **`commitrift discover` executes no application code**, with or without
  `--model-check`. It parses TypeScript, reads JSON files of the project
  (`tsconfig.json`, `package.json`, `commitrift.json`, the contracts file)
  and runs the model's engine on extracted code. The report says so
  (`executesApplicationCode: false`). Model checking explores models only.
* **Running original code** stays a separate command with the established
  authorization: `commitrift model-check <emitted scenario> --verify-source
  --allow-execution`, in the Phase 7–10 sandbox (a private disposable
  PostgreSQL cluster, `sandbox-exec` without IP networking on macOS,
  bubblewrap on Linux). Without `--allow-execution` the run is blocked; a
  test checks this for an emitted scenario
  (`generated_scenarios_never_run_application_code_without_authorization`).
  Discovery's emitted `sourceVerification` sections come only from the
  harness mapping the developer declared.
* **No third-party application was executed.** Only the first-party
  commerce application ran (`reports/phase11/evidence/`). The corpus was
  analyzed and model-checked statically.
* **Generated scenarios** carry synthesized, non-secret values only
  (`alice@example.test`, small integers). They inherit no environment
  variables; the runtime harness receives only what Phases 7–10 pass it.
* **Paths**: the discovery report names the project by its directory name
  and every location relative to the project root. Emitted scenarios refer
  to the project root by a relative path from the output directory: emit
  into a directory inside the repository to avoid recording the location of
  a private checkout (committed evidence does this). A test checks that the
  report contains no absolute path of the machine.
* **Private corpus**: committed reports for school-ledger and
  unnamed-ai-project contain aggregate counts only — no selectors, no
  source excerpts, no table names. The pinned revisions were extracted with
  `git archive` into a scratch directory; the private working trees were
  not modified.
* **HTML**: the discovery page is script-free with a restrictive
  Content-Security-Policy; every string is escaped. A test poisons source
  literals with `<script>` and checks the page.
