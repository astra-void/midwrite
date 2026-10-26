# Phase 8 original-code evidence (3 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.1 |
| accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.8 |
| checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.2 |
| checkout-idempotent-crash-after-charge | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.1 |
| checkout-idempotent-two-requests | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 3.9 |
| inventory-notify-after-commit | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 3.9 |
| inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 3.9 |
| inventory-retry-exhausted | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | — | 15.19 (Debian 15.19-0+deb12u1) | 3.8 |
| job-claimed-effect-concurrent | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.0 |
| job-claimed-effect-redelivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.0 |
| job-concurrent-claims-duplicate | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.1 |
| job-redelivery-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.0 |
| pgjs-accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.2 |
| pgjs-accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.1 |
| pgjs-accept-invitation-twice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 15.19 (Debian 15.19-0+deb12u1) | 4.0 |
| pgjs-checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.2 |
| pgjs-inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.0 |
| pgjs-invite-double-submit | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 15.19 (Debian 15.19-0+deb12u1) | 4.0 |
| shipping-dual-write-crash | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 3.1 |
| shipping-outbox-no-loss | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.0 |
| shipping-outbox-relay-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.9 |
