# Closed Alpha bug report

One report per problem. Do not include secrets, credentials or customer
data. For private code, reduce the problem to a small first-party example
if you can.

## Kind

- [ ] false positive (a reported counterexample that cannot happen on PostgreSQL)
- [ ] false negative (a real hazard reported as `NO_VIOLATION_WITHIN_BOUNDS`)
- [ ] wrong `UNSUPPORTED` / `INCONCLUSIVE` reason
- [ ] crash, hang or resource exhaustion
- [ ] original-code verification disagrees with the model
- [ ] documentation is wrong
- [ ] other

## Environment

* CommitRift revision and `commitrift --version`:
* OS / architecture:
* Node, PostgreSQL server, Drizzle, zod versions:

## Reproduction

```bash
the exact command
```

* Candidate id and status:
* If a counterexample: attach the `--emit-scenarios` scenario and its
  `*.counterexample.json` (they contain synthesized rows and arguments,
  not your data — check before sending).
* Minimal source that shows it (or a description of the construct):

## Expected vs actual

* Expected (with the PostgreSQL / zod / JavaScript behavior you rely on):
* Actual:

## Severity for you

- [ ] blocks using CommitRift
- [ ] misleading result
- [ ] cosmetic
