// AUTHORIZED SOURCE-VERIFICATION HARNESS for the postgres-js ledger.
//
// The application creates its postgres-js client at module load from
// POSTGRES_URL (lib/db/drizzle.ts). The harness provides the URL (the run's
// disposable database) and coordinates the installed postgres-js module
// (version-checked) before the application loads. The payment functions are
// imported unchanged and are the same as the node-postgres variant's.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import postgres from "postgres";
import {
  coordinatePostgresJs,
  instrumentDrizzle,
  runSourceActor,
  sourceEffect,
  sourcePostgresUrl,
} from "@midwrite/replay/source";

import type { PayInput } from "../src/app/payments.js";
import { useConfirmations } from "../src/lib/messaging/confirmations.js";

const require = createRequire(import.meta.url);
const packageJson = join(dirname(require.resolve("postgres")), "..", "..", "package.json");
const version = JSON.parse(readFileSync(packageJson, "utf8")).version as string;

instrumentDrizzle(drizzle, pgCore);
coordinatePostgresJs(postgres, version);
process.env.POSTGRES_URL = sourcePostgresUrl();
useConfirmations({
  publish: (confirmation) =>
    sourceEffect("confirmation", {
      key: null,
      payload: { ...confirmation },
    }),
});

const { client } = await import("../src/lib/db/drizzle.js");
const payments = await import("../src/app/payments.js");

await runSourceActor(
  {
    recordPayment: (input: PayInput) => payments.recordPayment(input),
    recordPaymentIdempotent: (input: PayInput) => payments.recordPaymentIdempotent(input),
    recordPaymentReconciled: (input: PayInput) => payments.recordPaymentReconciled(input),
    recordPaymentAutocommit: (input: PayInput) => payments.recordPaymentAutocommit(input),
    recordPaymentRetryInProcess: (input: PayInput) => payments.recordPaymentRetryInProcess(input),
    recordPaymentIdempotentRetryInProcess: (input: PayInput) =>
      payments.recordPaymentIdempotentRetryInProcess(input),
    recordPaymentReconciledRetryInProcess: (input: PayInput) =>
      payments.recordPaymentReconciledRetryInProcess(input),
    recordPaymentAutocommitRetryInProcess: (input: PayInput) =>
      payments.recordPaymentAutocommitRetryInProcess(input),
  },
  // Process shutdown, not application logic. Observed in Phase 10: with this
  // harness's connections going through the fault proxy, `client.end()`
  // without a timeout did not resolve, also in processes whose connections
  // had no fault; the Phase 8 postgres-js harness, which does not use the
  // proxy, ends normally. The cause was not established
  // (docs/architecture/fault-injection.md). postgres-js's own `timeout` option
  // ends the remaining connections after 5 s, and the harness says so on
  // stderr when it took that long.
  async () => {
    const started = Date.now();
    await client.end({ timeout: 5 });
    const ms = Date.now() - started;
    if (ms >= 4000) console.error(`cleanup: client.end() needed its timeout (${ms} ms)`);
  },
);
