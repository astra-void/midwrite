// AUTHORIZED SOURCE-VERIFICATION HARNESS for the shop application.
//
// It runs the application's ORIGINAL exported functions the way the server
// would: with the database pool and the payment-provider client configured
// at process start. The checkout code is imported unchanged. What the
// harness provides is what the process entry point provides:
//
//   * the database pool (coordinated by Midwrite);
//   * the payment provider, the event bus and the mailer: TEST DOUBLES whose
//     calls are received by the Midwrite coordinator (`sourceEffect`). A
//     double's deduplication is the scenario's DECLARED channel contract —
//     an assumption about the real provider, not evidence about it.
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourceEffect,
  sourcePoolConfig,
} from "@midwrite/replay/source";

import type { PayRequest } from "../src/app/checkout.js";
import type { ReserveRequest } from "../src/app/inventory.js";
import type { JobDelivery } from "../src/app/jobs.js";
import type { ShipRequest } from "../src/app/shipping.js";
import { usePool } from "../src/lib/db/pool.js";
import { useMessaging } from "../src/lib/messaging/bus.js";
import { usePaymentProvider } from "../src/lib/payments/provider.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
usePool(pool);
usePaymentProvider({
  charge: (request, idempotencyKey) =>
    sourceEffect("card-charge", { key: idempotencyKey ?? null, payload: { ...request } }),
});
useMessaging({
  publish: (event, messageId) =>
    sourceEffect("order-events", { key: messageId ?? null, payload: { ...event } }),
  sendEmail: (receipt) => sourceEffect("receipt-email", { payload: { ...receipt } }),
  notifyWarehouse: (notice) => sourceEffect("warehouse", { payload: { ...notice } }),
});

// Loaded after the seams are configured (`db` is created at module load).
const checkout = await import("../src/app/checkout.js");
const shipping = await import("../src/app/shipping.js");
const jobs = await import("../src/app/jobs.js");
const inventory = await import("../src/app/inventory.js");

await runSourceActor(
  {
    payOrder: (input: PayRequest) => checkout.payOrder(input),
    payOrderIdempotent: (input: PayRequest) => checkout.payOrderIdempotent(input),
    shipOrderDualWrite: (input: ShipRequest) => shipping.shipOrderDualWrite(input),
    shipOrderWithOutbox: (input: ShipRequest) => shipping.shipOrderWithOutbox(input),
    relayOutbox: () => shipping.relayOutbox(),
    sendReceipt: (input: JobDelivery) => jobs.sendReceipt(input),
    sendReceiptOnce: (input: JobDelivery) => jobs.sendReceiptOnce(input),
    reserveStock: (input: ReserveRequest) => inventory.reserveStock(input),
    reserveStockNotifyAfterCommit: (input: ReserveRequest) =>
      inventory.reserveStockNotifyAfterCommit(input),
    reserveStockOnce: (input: ReserveRequest) => inventory.reserveStockOnce(input),
  },
  () => pool.end(),
);
