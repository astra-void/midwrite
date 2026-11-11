// AUTHORIZED SOURCE-VERIFICATION HARNESS for the Phase 14 request
// application. It calls the application's ORIGINAL exported functions as
// its server would. The coordinator plays the HTTP client.
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourcePoolConfig,
} from "@midwrite/replay/source";

import { usePool } from "../src/db/pool.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
pool.on("error", () => {});
// ENVIRONMENT ASSUMPTION (stated): like a server that follows the
// node-postgres guidance, the harness handles errors of checked-out clients.
// Without this handler a broken connection during a transaction's COMMIT
// emits an unhandled 'error' event and the process dies (observed: Phase 14
// evidence `default-harness/`), so the application's retry never runs.
pool.on("connect", (client) => client.on("error", () => {}));
usePool(pool);

const items = await import("../src/handlers/items.js");
const payments = await import("../src/services/payments.js");

await runSourceActor(
  {
    addOrderItems: (input: { orderId: number; requestId: string }) => items.addOrderItems(input),
    addOrderItemsIdempotent: (input: { orderId: number; requestId: string }) =>
      items.addOrderItemsIdempotent(input),
    settlePayment: (input: { orderId: number; amount: number }) =>
      payments.settlePayment(input.orderId, input.amount),
    settlePaymentOnce: (input: { orderId: number; amount: number }) =>
      payments.settlePaymentOnce(input.orderId, input.amount),
  },
  () => pool.end(),
);
