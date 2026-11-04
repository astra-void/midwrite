// AUTHORIZED SOURCE-VERIFICATION HARNESS for the Phase 16 retry
// application. It calls the application's ORIGINAL exported functions (and
// its route handler, with a Fetch `Request` built from the JSON body) as its
// server would, one process per actor.
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourcePoolConfig,
} from "@commitrift/replay/source";

import { usePool } from "../src/db/pool.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
pool.on("error", () => {});
// ENVIRONMENT ASSUMPTION (stated): like a server that follows the
// node-postgres guidance, the harness handles errors of checked-out clients;
// without it a broken connection emits an unhandled 'error' event and the
// process dies before the application's retry loop runs (Phase 14 evidence).
pool.on("connect", (client) => client.on("error", () => {}));
usePool(pool);

const orders = await import("../src/services/orders.js");
const route = await import("../src/app/api/orders/route.js");

type Order = { requestId: string; customerId: number; amount: number };

await runSourceActor(
  {
    placeOrder: (input: Order) => orders.placeOrder(input.requestId, input.customerId, input.amount),
    recordPayment: (input: { idempotencyKey: string; orderId: number; amount: number }) =>
      orders.recordPayment(input.idempotencyKey, input.orderId, input.amount),
    withdraw: (input: { accountId: number; amount: number }) =>
      orders.withdraw(input.accountId, input.amount),
    POST: async (input: { body: unknown }) => {
      const response = await route.POST(
        new Request("http://localhost/api/orders", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(input.body),
        }),
      );
      return { status: response.status, body: await response.json() };
    },
  },
  () => pool.end(),
);
