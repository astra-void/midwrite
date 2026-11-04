# Closed Alpha feedback

Copy this file, fill it in, and send it back the way your invitation says.
Do not include secrets, credentials or customer data; for private code,
describe constructs instead of pasting source.

## You and your project

* Stack (framework, Drizzle version, zod version, PostgreSQL version):
* Size (approximate number of server functions / route handlers):
* CommitRift revision (`git rev-parse --short HEAD`) and OS:

## What you ran

```text
commands, exactly as typed
```

## Results

| candidate id | status | property origin | was it right? (yes / no / unsure) | why |
| --- | --- | --- | --- | --- |
| | | | | |

* Findings you would act on:
* Findings you consider wrong (false positives) — please also file a
  [bug report](bug-report-template.md):
* Hazards you know of that were not reported (false negatives):
* First `UNSUPPORTED` blockers you saw most often:

## Usability

* Could you tell from the output where the problem is in your source? (1–5)
* Could you tell whether a property was declared or guessed? (1–5)
* Did the coverage section make clear what was not explored? (1–5)
* Did you try `--emit-scenarios` / `model-check` on a counterexample? Did
  the schedule make sense?
* Did you declare a property in `commitrift-contracts.json`? What was hard?
* Did you run original-code verification (`--verify-source
  --allow-execution`)? With which harness effort?

## Anything else
