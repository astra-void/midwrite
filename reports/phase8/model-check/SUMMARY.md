# Phase 8 original-code evidence (5 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | — | yes | 17.11 (Homebrew) | 12.8 |
| accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | — | yes | 17.11 (Homebrew) | 11.5 |
| checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 5/5 / 5/5 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 9.9 |
| checkout-idempotent-crash-after-charge | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 10.1 |
| checkout-idempotent-two-requests | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | violated | — | — | yes | 17.11 (Homebrew) | 9.3 |
| inventory-notify-after-commit | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 5/5 / 5/5 | holds | — | — | yes | 17.11 (Homebrew) | 8.9 |
| inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | violated | — | — | yes | 17.11 (Homebrew) | 8.7 |
| inventory-retry-exhausted | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 5/5 / 5/5 | holds | — | — | — | 17.11 (Homebrew) | 8.7 |
| job-claimed-effect-concurrent | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 5/5 / 5/5 | holds | — | — | yes | 17.11 (Homebrew) | 8.5 |
| job-claimed-effect-redelivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 9.0 |
| job-concurrent-claims-duplicate | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | violated | — | — | yes | 17.11 (Homebrew) | 9.0 |
| job-redelivery-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 5/5 / 5/5 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 9.3 |
| pgjs-accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | — | yes | 17.11 (Homebrew) | 10.4 |
| pgjs-accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | — | yes | 17.11 (Homebrew) | 10.4 |
| pgjs-accept-invitation-twice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | violated | — | — | — | 17.11 (Homebrew) | 8.9 |
| pgjs-checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 5/5 / 5/5 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 9.3 |
| pgjs-inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | violated | — | — | yes | 17.11 (Homebrew) | 8.8 |
| pgjs-invite-double-submit | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | violated | — | — | — | 17.11 (Homebrew) | 9.2 |
| shipping-dual-write-crash | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 5/5 / 5/5 | violated | agree | — | yes | 17.11 (Homebrew) | 7.9 |
| shipping-outbox-no-loss | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 5/5 / 5/5 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 10.4 |
| shipping-outbox-relay-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 5/5 / 5/5 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 10.4 |
