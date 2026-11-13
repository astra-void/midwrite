# Release status (DEV-164)

What has actually happened for each stage of a Midwrite release, with the
check that shows it. **Checked on 2026-11-13**, with `main` at `b9027d2`.
Re-run the commands in each row before relying on a status. Any statement
elsewhere in the documentation that contradicts this page is stale.

## v0.1.0

| stage | status | evidence |
| --- | --- | --- |
| Source committed and public | **yes** | [astra-void/midwrite](https://github.com/astra-void/midwrite), public, created 2026-11-11; `gh repo view astra-void/midwrite` |
| Version tag | **yes** | `v0.1.0` → `3e66c158c30a`; `git rev-list -n1 v0.1.0` |
| Release workflow configured | **yes** | [`.github/workflows/release.yml`](../../.github/workflows/release.yml) |
| Tagged release run passed | **yes** | [run 43151652843](https://github.com/astra-void/midwrite/actions/runs/43151652843), 2026-11-12, at `3e66c15`. Full test suite on macOS arm64 and x64 and on Linux x64 and arm64 (runner host targets). Smoke test on all six shipped binaries, and on Windows only the smoke test |
| CI on the release commit | **no** | [ci run 43151652960](https://github.com/astra-void/midwrite/actions/runs/43151652960) at `3e66c15` failed in required replay (`the_completed_generated_harness_reproduces_the_drizzle_dual_write`) |
| CI on `main` now | **not reliably green** | See [CI on main](#ci-on-main) |
| GitHub Release published | **yes** | [v0.1.0](https://github.com/astra-void/midwrite/releases/tag/v0.1.0), published 2026-11-12, not a pre-release, six archives with a `.sha256` file each; `gh release view v0.1.0` |
| npm packages published | **yes, by hand** | `midwrite`, `@midwrite/cli-{darwin-arm64,darwin-x64,linux-arm64,linux-x64,win32-x64,win32-arm64}` and `@midwrite/replay`, all `0.1.0`, published 2026-11-12 by the maintainer's account without provenance attestations. `midwrite` also has a `0.0.0-stage` version. `npm view midwrite versions` |
| Clean install verified | **yes, all six platforms** | [install-check run 43361753653](https://github.com/astra-void/midwrite/actions/runs/43361753653), 2026-11-13. On a fresh runner per platform, `npm install --save-dev midwrite@0.1.0` installed the matching platform package, `npx midwrite --version` printed `midwrite 0.1.0`, and `npx midwrite check fixtures/vulnerable` exited 1 with 70 diagnostics. `@midwrite/replay` installed, and its four entry points imported on Node 22 |
| Release checksums | **match** | The same run downloaded each archive; its SHA-256 equals the published `.sha256` file on all six platforms |
| npm binary = release archive binary | **macOS and Linux yes, Windows no** | Byte-identical on darwin-arm64, darwin-x64, linux-arm64 and linux-x64. The Windows x64 and arm64 binaries differ between npm and the archive: they were built in different runs. Each one passes the smoke test |
| crates.io | **not published** | `https://crates.io/api/v1/crates/midwrite` returns 404 |

## CI on main

[`ci.yml`](../../.github/workflows/ci.yml) runs on every push to `main` and
on pull requests. It runs:

* fmt and clippy;
* static tests on `ubuntu-24.04` (x64) and `macos-15` (arm64);
* the evaluation gate;
* required replay with bubblewrap on `ubuntu-24.04`.

Of the 29 completed push runs from 2026-11-11 to 2026-11-13, 10 passed, 17
failed and 4 were cancelled. Nearly every failure is a required-replay
runtime test:

| test | failed runs | last failure |
| --- | ---: | --- |
| `the_completed_generated_harness_reproduces_the_drizzle_dual_write` (`crates/midwrite-cli/tests/replay.rs`) | 10 | 2026-11-12 10:36 |
| `a_severed_connection_with_a_discarded_commit_rolls_the_transaction_back` (`crates/midwrite-replay/tests/proxy_postgres.rs:144`, "timed out: the backend ended") | 3 | 2026-11-13 02:48 |
| other replay tests, single occurrences | 2 | 2026-11-12 |
| non-test step failures | 2 | 2026-11-11 |

The second test fails intermittently. It failed on commits that changed
only labels (`6e35f85`) and on commits that passed on a re-run of the same
code (`7f74be7`). The static-analysis jobs and the evaluation gate passed
on every run listed. "CI is green" is therefore not a claim the project can
make today. `gh run list --workflow ci.yml` shows the current state.

**Cause of the second failure, fixed on 2026-11-13.** The fault proxy
published `OperationHeld` before it stored the withheld COMMIT. A test that
severed and discarded right after the event could clear an empty buffer.
The COMMIT was stored afterwards, so the proxy kept the server connection
open for a delivery that never came, and the backend stayed `idle in
transaction` until the deadline. The proxy now stores the frame before it
publishes the event, and a discarded connection is never kept. Regression
tests are in `crates/midwrite-replay/src/proxy.rs`. Whether CI on `main` is
reliably green after the fix depends on the runs that follow.

## Re-checking

```bash
gh workflow run install-check.yml -R astra-void/midwrite -f version=0.1.0
```

```bash
gh run list -R astra-void/midwrite --workflow ci.yml -L 30
```

```bash
npm view midwrite versions
```

The install check builds nothing. It installs from npm and downloads the
GitHub release exactly as a user would. Each platform job uploads a
`result.json` with the archive and binary digests.

## Not covered

* No install has been checked with pnpm, Yarn or Bun, or with
  `--omit=optional`.
* Only Node 22 was used. The launcher declares `node >= 18`; that floor is
  untested.
* Linux distributions other than the GitHub Ubuntu 24.04 runners are
  untested. The binaries are static musl.
* The independent evaluation is still AI-labeled. The current held-out
  numbers are in the [README](../../README.md#what-a-warning-means),
  and the status of the human review is in [human review](human-review.md).
