// AUTHORIZED SOURCE-VERIFICATION HARNESS for the commerce application.
//
// It runs the application's ORIGINAL exported functions the way the server
// or a worker would: Server Actions inside a request with a session and a
// FormData body, services and the queue consumer with their inputs. What
// the harness provides is what the process entry point provides:
//
//   * the database pool (coordinated by Midwrite);
//   * the payment provider: a TEST DOUBLE whose calls are received by the
//     Midwrite coordinator (`sourceEffect`); its deduplication is the
//     scenario's DECLARED channel contract, not evidence about a provider.
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

import type { ReserveRequest } from "../src/app/inventory.js";
import { usePool } from "../src/db/pool.js";
import { runWithSession } from "../src/lib/session.js";
import { usePaymentProvider } from "../src/services/payments.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
// node-postgres emits 'error' on a client whose connection ends; without a
// listener the process dies (Phase 10).
pool.on("error", () => {});
usePool(pool);
usePaymentProvider({
  charge: (request, idempotencyKey) =>
    sourceEffect("card-charge", { key: idempotencyKey ?? null, payload: { ...request } }),
  payout: (request) => sourceEffect("payout", { payload: { ...request } }),
});

// Loaded after the seams are configured (`db` is created at module load).
const invitations = await import("../src/app/invitations.js");
const inventory = await import("../src/app/inventory.js");
const checkout = await import("../src/app/checkout.js");
const jobs = await import("../src/services/jobs.js");
const orders = await import("../src/services/orders.js");

type FormInput = { userId: number; form: Record<string, string> };

function formData(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [key, value] of Object.entries(fields)) data.set(key, value);
  return data;
}

await runSourceActor(
  {
    inviteMember: (input: FormInput) =>
      runWithSession({ userId: input.userId }, () =>
        invitations.inviteMember({}, formData(input.form)),
      ),
    inviteSeat: (input: FormInput) =>
      runWithSession({ userId: input.userId }, () => invitations.inviteSeat({}, formData(input.form))),
    reserveItem: (input: ReserveRequest) => inventory.reserveItem(input),
    reserveItemAtomic: (input: ReserveRequest) => inventory.reserveItemAtomic(input),
    reserveItemLocked: (input: ReserveRequest) => inventory.reserveItemLocked(input),
    checkout: (input: { requestId: string; orderId: number }) => checkout.checkout(input),
    checkoutIdempotent: (input: { requestId: string; orderId: number }) =>
      checkout.checkoutIdempotent(input),
    processRefund: (input: { jobId: number }) => jobs.processRefund(input),
    processRefundOnce: (input: { jobId: number }) => jobs.processRefundOnce(input),
    claimTask: (input: { taskId: number; assignee: string }) => orders.claimTask(input),
    claimTaskConditional: (input: { taskId: number; assignee: string }) =>
      orders.claimTaskConditional(input),
  },
  () => pool.end(),
);
