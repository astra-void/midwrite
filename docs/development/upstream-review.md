# Upstream review (DEV-150)

On 2026-11-11 Midwrite scanned five real TypeScript backends outside the
Midday dev set. Every diagnostic in both the initial and final runs has a
[source-review record](../../corpora/upstream/reviews/). **Four distinct
source-reviewed bugs were reported upstream**, meeting the stage-2
report-count gate (at least three reports):

| project | report | failure and recovery consequence |
| --- | --- | --- |
| Kan | [#697](https://github.com/kanbn/kan/issues/697) | Workspace creation commits before subscription creation. Redelivery finds the workspace and skips the missing subscription. |
| Rybbit | [#1355](https://github.com/rybbit-io/rybbit/issues/1355) | Lifecycle/trial-grace claims survive a crash before delivery and suppress subsequent sends. Usage-limit notification flags also survive an ordinary failed send. |
| Vercel chatbot | [#1567](https://github.com/vercel/chatbot/issues/1567) | Dependent deletes commit separately; an error can leave a visible chat whose messages were already deleted. |
| OpenStatus | [#3077](https://github.com/openstatusHQ/openstatus/issues/3077) | Signup commits user, workspace and owner membership separately, allowing a persisted user without their initial owned workspace. |

Each issue includes pinned source links, a proposed failure-injection
sequence, the expected/actual state, a mitigation, a Midwrite link, and the
limits of the evidence. The Midwrite project URL was supplied by the user;
it returned HTTP 404 during lookup, so public availability of that attribution
link remains unverified. The strongest example, Kan's retry-skipping guard,
now appears at the top of the [README](../../README.md).

**These are AI-assisted manual source reviews, not independent human
review or runtime reproduction.** No external application code was executed,
no dependencies were installed, and no external checkout was edited. A
reported issue is not maintainer confirmation. Human review and executed
reproductions remain future evidence; the gate met here is the count of
upstream reports. Other candidates with unresolved intent/recovery are
explicitly `needs-triage`, not counted as confirmed bugs or safe results.
On 2026-11-13 all four issues were still open with no maintainer reply
([status](../../corpora/review/upstream-status.json)), so every report is
unconfirmed. The four reports are tier-1 items of the
[human review](human-review.md) sample.

## Repositories and complete scan results

Pinned revisions, input-tree digests, binary digests, configurations, exit
codes and screening decisions are in the
[manifest](../../corpora/upstream/manifest.json). Full JSON output is in
[`reports/upstream`](../../reports/upstream/).

| repository | revision | files analyzed | hazards before → after | final MW9001 notices |
| --- | --- | ---: | ---: | ---: |
| vercel/chatbot | `a8989b522fa3672a2ae52f053105e40993b36ca1` | 154 | 9 → 9 | 3 |
| rybbit-io/rybbit | `4288204761ed2fc801fcaa0b3bbfacd1b8cc3936` | 385 | 35 → 28 | 70 |
| openstatusHQ/openstatus | `36288495c8544d4498986be7d72c37ee7e666837` | 2,544 | 83 → 70 | 59 |
| kanbn/kan | `1cc2f332131d736c5e262654bd279e3beb95c93d` | 374 | 91 → 91 | 39 |
| dittofeed/dittofeed | `91277939a9afcee797ab2562025f6b02d823b00c` | 594 / 595 discovered | 21 → 21 | 40 |

Hazards are MW1001–MW1005 at every severity, including informational
MW1004/MW1005. MW9001 describes missing coverage, not a defect claim.
Dittofeed has one parse failure in `controlled-select-reference.ts` (JSX
in a `.ts` file, line 37), exits 3 even with `--fail-on never`, and is
**an incomplete scan**. It is not counted as a clean scan. OpenStatus uses
libSQL: this pass inspects Drizzle effect ordering only and does not apply
PostgreSQL modeling or claim dialect/runtime verification.

OpenPanel and Plunk were screened from pinned source but use Prisma;
neither is counted among the five scans or as a negative result.

Kan and Dittofeed already belong to the frozen held-out evaluation. Their
configuration and labels were copied for reference; the original files,
set membership and freeze record are unchanged. Their earlier AI verdicts
are identified in each review record. The new HTTP fix was motivated by
Rybbit/OpenStatus, not a held-out finding. This bug hunt is not a new blind
precision/recall evaluation, and no accuracy score is claimed.

## Repeated false-positive fix

The initial broad `fetch` rule treated HTTP reads as independent mutations.
The new optional [`externalEffects[].httpMethods`](../reference/configuration.md#narrowing-http-effects)
filter restricts a fetch-style rule to an explicit string-literal method in
its second argument. Existing configurations keep their behavior. An
unresolved options variable, spread override, getter, computed key or
`Request` object remains unknown; it is never silently declared pure.

Final configurations remove **20 source-reviewed read-related false
positives**: seven in Rybbit and thirteen in OpenStatus. These include
configuration/font downloads, provider metadata/license lookups, certificate
readiness, MCP health probes and recording/backfill reads. Four changing
OpenStatus fingerprints represent the same domain/Teams writes recognized
through explicit helper rules, not four more removed false positives.
Known writes behind wrappers remain configured (`safeFetch` with a literal
method; Vercel mutators by their imported function names).

This does not solve every false-positive family. A read-only POST query
still matches a write-method rule. Source-reviewed residual false positives
also include mutually exclusive property branches, an unsuccessful CAS
update preceding a retry, a standalone helper always called under a
transaction, and unknown-return/compensation correlations. The review
records retain these limits; they are not removed with project-specific
source suppressions or post-result exclusions.

Regression coverage includes read and write counterparts, dynamic methods,
spread ordering, overwritten methods, getters, computed keys, spread call
arguments, validation/serialization, and unchanged broad-rule behavior.

## Reproduce and check completeness

```bash
cargo build --release -p midwrite-cli
python3 scripts/upstream-review.py fetch
python3 scripts/upstream-review.py run --phase before
python3 scripts/upstream-review.py run --phase after
python3 scripts/upstream-review.py verify
```

`fetch` downloads pinned source archives into ignored `.corpora/upstream/`
paths and verifies their tree digests. It refuses a changed existing tree.
`run` checks source digests before and after static analysis and writes new
reports under `.corpora/upstream/rerun/`; it does not overwrite the original
reports. The before/after runs use their respective saved configurations.
Only the HTTP rule/configuration changes apply to this campaign; the
initial binary and final binary digests are recorded separately.

`verify` checks that every diagnostic fingerprint in both saved runs has
exactly one per-phase review record with a classification and explanation,
and that every `reported` finding links to an upstream report. It checks
four locally recorded distinct upstream issue URLs against the gate; it
does not assert that maintainers accepted them. Issue publication and
Midwrite attribution were also read back from GitHub after posting.

Review classes are `reported`, `false-positive`, `intended-risk`,
`needs-triage`, and `analysis-limitation`. Multiple diagnostics can refer to
one root bug; they never inflate the distinct-report gate. Complete records
include unresolved candidates and removed diagnostics, so selecting the
four best reports does not discard the rest of the scan.

## Validation

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Release build: passed.
- Analyzer fixtures, HTTP regression tests, and all 27 CLI tests: passed.
- Documentation links: no broken links in the checked documentation.
- Five-repository reruns: source digests unchanged; every baseline summary,
  diagnostic and parse failure is identical using the final binary and the
  saved original configuration. Before/after differences therefore come
  from the opt-in configuration change, with unchanged broad-rule behavior.
- The full workspace test run was attempted. Runtime tests encountered
  sandbox restrictions on PostgreSQL shared memory/process isolation. A
  retry outside the sandbox also hit the host's exhausted shared-memory
  limit (`shmget: No space left on device`, explicitly not disk space).
  These runtime checks have not passed; the broad run was stopped after
  recording these failures while unrelated model stress tests continued. No
  external-repository runtime claim depends on them.
