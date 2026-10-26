// AUTHORIZED SOURCE-VERIFICATION HARNESS for the ledger application.
//
// It runs the application's ORIGINAL exported functions the way the server
// would: with the database pool and the confirmation client configured at
// process start. The payment code is imported unchanged. The confirmation
// client is a TEST DOUBLE whose calls are received by the CommitRift
// coordinator (`sourceEffect`); its deduplication is the scenario's DECLARED
// channel contract.
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourceEffect,
  sourcePoolConfig,
} from "@commitrift/replay/source";

import type { PayInput } from "../src/app/payments.js";
import { usePool } from "../src/lib/db/pool.js";
import { useConfirmations } from "../src/lib/messaging/confirmations.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
usePool(pool);
useConfirmations({
  publish: (confirmation) =>
    sourceEffect("confirmation", {
      key: null,
      payload: { ...confirmation },
    }),
});

// Loaded after the seams are configured (`db` is created at module load).
const payments = await import("../src/app/payments.js");

await runSourceActor(
  {
    recordPayment: (input: PayInput) => payments.recordPayment(input),
    recordPaymentIdempotent: (input: PayInput) => payments.recordPaymentIdempotent(input),
    recordPaymentReconciled: (input: PayInput) => payments.recordPaymentReconciled(input),
    settleOrder: (input: PayInput) => payments.settleOrder(input),
    recordPaymentAutocommit: (input: PayInput) => payments.recordPaymentAutocommit(input),
    settleWithAudit: (input: PayInput) => payments.settleWithAudit(input),
    notifyPayment: (input: PayInput) => payments.notifyPayment(input),
  },
  () => pool.end(),
);
