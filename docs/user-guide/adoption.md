# Adopting `midwrite check`

A short path from the first scan to a CI check that fails only on new
findings. Reference for every step: [`midwrite check`](check.md),
[configuration](../reference/configuration.md),
[diagnostics](../DIAGNOSTICS.md).

Findings are potential hazards found by static analysis. None is a
reproduced failure, and a run without findings is not proof that the code
is safe ([limitations](../reference/limitations.md)).

## 1. First scan

```bash
midwrite check .
```

Without a configuration Midwrite recognizes a database receiver named
`db`, `queue.publish` and the built-in SDK clients. If your handle or your
effects have other names, the first scan is mostly `MW9001` notices
("analysis incomplete"). Describe them in `midwrite.json`:

```json
{
  "databaseReceivers": ["db", "ctx.db"],
  "externalEffects": [
    { "receiver": "mailer", "method": "send", "category": "email" }
  ]
}
```

Then read the summary lines at the end:

* `analyzed N of M file(s)`: files that failed to parse are listed as
  errors and are not analyzed.
* `analysis incomplete at N reported location(s) (MW9001)`: places
  Midwrite does not understand. No finding there is not a clean result.
* `N finding(s) repeat an effect pair ...`: the same pair reached from
  several functions, shown once.

## 2. Review the findings

Each finding names, in fixed places
([reading a finding](../DIAGNOSTICS.md#reading-a-finding-in-the-text-output)):

* `operations:` the effect that is already durable (`completed first`) and
  the one that is left undone (`not yet performed`), with locations;
* `basis:` how certain the path is and how the operations were recognized;
* `recovery:` what the configuration declares, or `none declared`;
* `analysis gaps:` whether the same function has unanalyzed operations.

For each finding decide one of three things:

| decision | what to do |
| --- | --- |
| It is a bug | Fix it (the `help:` line names the usual fix), or keep it in the baseline until it is fixed. |
| The window is real but handled outside this function | Declare the recovery (step 4). The finding stays visible as `info`. |
| The analysis is wrong for this code | Suppress that line with a reason ([inline suppressions](check.md#inline-suppressions)) and consider reporting it. |

Start with `warning` findings at `high` confidence. `info` findings
(`MW1004`, `MW1005`) are windows between two operations of the same kind
and are often accepted.

## 3. Record a baseline

```bash
midwrite check . --write-baseline midwrite-baseline.json
git add midwrite-baseline.json midwrite.json
```

The baseline lists every current finding by fingerprint. Fingerprints do
not contain line numbers, so moving code does not invalidate them
([details](check.md#baselines-adopting-it-in-ci)). A baseline is a list of
known findings, not a list of accepted ones: review continues from it.

## 4. Declare a recovery, with evidence

When something outside the function repairs the window (a queue that
retries the whole job, a reconciliation task), say so where a reviewer
can check it:

```json
{
  "recoveryContracts": [{
    "function": "ProcessDocumentProcessor.process",
    "file": "apps/worker/src/processors/process-document.ts",
    "strategy": "retried",
    "covers": ["MW1001"],
    "evidence": "queues/documents.config.ts:17 attempts 3; stalled jobs are re-run",
    "limits": "gives up after 3 attempts"
  }]
}
```

* `function` is matched exactly, and `covers` lists codes explicitly.
  There are no patterns and no blanket contracts.
* `evidence` should point at the code or configuration that implements
  the mechanism. `limits` says where it stops.
* The finding is still reported, as `info`, with the declaration,
  `verified: false` and a note. Midwrite does not check that the
  mechanism exists or covers this window. A `retried` function runs its
  first effect again, so that effect must be idempotent.

Use `externalEffects[].recovery` (`bestEffort`, `reconciled`,
`idempotentConsumer`) for a property of one effect everywhere it is used
([recovery declarations](../reference/configuration.md#recovery-declarations)).
Recovery is never inferred from names or from a `try`/`catch`.

## 5. Run it in CI

```bash
midwrite check . --baseline midwrite-baseline.json
```

The run fails (exit status 1) only when a finding at or above `--fail-on`
(default `warning`) is not in the baseline. Findings in the baseline are
still printed, marked `(in baseline)`.

When CI reports a finding marked `(new)`:

1. Read its `operations:` lines. A new finding usually means the change
   added a write or an external call next to an existing one, or moved one
   out of a transaction.
2. Decide as in step 2: fix it, declare a recovery, or suppress the line
   with a reason. Each of these is a reviewable change in the pull request.
3. Do not regenerate the baseline to make CI pass. Regenerate it only
   after reviewing what it would add, for example after an upgrade that
   reports more (`midwrite check . --baseline old.json` lists what is new
   first).

For GitHub code scanning, upload SARIF instead
([workflow](check.md#github-code-scanning)).

## What the evidence behind the defaults is

Precision and recall figures in this repository come from labels written
and re-reviewed by AI agents, not by people
([evaluation harness](../development/evaluation-harness.md),
[review record](../development/human-review.md)). Treat severities and
confidence as triage order, not as measured probabilities for your code.
