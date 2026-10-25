# Contributing to CommitRift

Thank you for helping. CommitRift is experimental; correctness and honest
reporting matter more than coverage.

## Ground rules

* **No unearned confidence.** A diagnostic must be backed by recognized
  operations and a path in the supported model. If the analysis cannot decide,
  it reports `CP9001` or stays silent; it never guesses an effect.
* **Every behavior change comes with a fixture.** Add a file under
  `fixtures/vulnerable/`, `fixtures/safe/` or `fixtures/unsupported/` with
  `// @expect` annotations, for example:

  ```ts
  // @expect CP1001 line=12 confidence=high function=completeOrder
  // @expect none
  // @expect parse-error
  ```

  `crates/commitrift-analyzer/tests/fixtures.rs` checks that the set of
  `(code, line)` pairs matches exactly.
* **Keep the core independent.** `commitrift-core` must not depend on a parser.
  Frontend concerns (Oxc, TypeScript syntax, Drizzle chains) live in
  `commitrift-analyzer`.
* **Keep output deterministic.** No hash-map iteration in anything that reaches
  the report.
* **Document limitations.** If a change adds an approximation, list it in
  `docs/LIMITATIONS.md`.

## Before opening a pull request

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

Useful while developing: `cargo run -p commitrift-cli -- ir <file>` prints the
Effect IR the frontend produced for a file, and `-- trace <file>` the failure
trace behind each finding.

Runtime replay tests (`crates/commitrift-cli/tests/replay.rs`) run the
harnesses under `fixtures/replay/`. They need Bun, Node.js ≥ 22, the
PostgreSQL server binaries, an isolation backend (macOS `sandbox-exec`, or
Linux bubblewrap with unprivileged user namespaces) and the Drizzle fixture's
pinned dependencies (`cd fixtures/replay/drizzle-node && npm ci`); without
them they print `SKIPPED` and pass, unless `COMMITRIFT_REQUIRE_REPLAY=1` is
set. A skipped replay test is not runtime verification.
`scripts/linux-replay-docker.sh` runs fmt, clippy and the full suite with
required replay on Linux in a container. A change to replay should keep
`scripts/demo.sh` and `scripts/demo-phase4.sh` working, and the vendored client of
`fixtures/replay/drizzle-node/generated-harness/commitrift-runtime/` in sync
with `runtime/node` (copy the files; do not run `init-replay --force` there,
which would also overwrite the completed manifest and harness). A test fails
when they drift.
Reports must stay schema-valid: `scripts/validate-schemas.py` (needs
`jsonschema`). Never add a test that executes code from
`.corpora/` or any external repository: corpus projects are analyzed
statically only.

## Licensing

By contributing you agree that your contributions are dual licensed under the
MIT license and the Apache License, Version 2.0, as described in the README.
