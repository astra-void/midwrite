// Replay entry point (compiled to dist/replay/harness.js and run by
// `commitrift replay` with Node.js). It is the only replay-aware code: it
// wires the application's pool and queue seams to the run's disposable
// database and event log, then loads the application unchanged.
import pg from "pg";
import { runScenarios } from "@commitrift/replay";
import { instrumentPgPool, replayPoolConfig } from "@commitrift/replay/pg";
import { publishToEventLog } from "@commitrift/replay/event-log";

import { usePool } from "../src/pool.js";
import { useTransport } from "../src/queue.js";

const pool = instrumentPgPool(new pg.Pool(replayPoolConfig()));
usePool(pool);
useTransport(publishToEventLog);

// Loaded only now: `db` is created from the configured pool at import time.
const { completeOrder, recordPayment, shipOrder, confirmOrder } = await import("../src/service.js");
const { relayOutbox } = await import("../src/relay.js");

type OrderInput = { orderId: string };

await runScenarios(
  {
    completeOrder: (input: OrderInput) => completeOrder(input.orderId),
    recordPayment: (input: OrderInput & { amountCents: number }) => recordPayment(input.orderId, input.amountCents),
    shipOrder: (input: OrderInput & { trackingNumber: string }) => shipOrder(input.orderId, input.trackingNumber),
    // Like an HTTP handler: a failed request is answered with an error
    // response, the process keeps running.
    shipOrderRequest: async (input: OrderInput & { trackingNumber: string }) => {
      try {
        await shipOrder(input.orderId, input.trackingNumber);
      } catch (error) {
        const cause = error instanceof Error && error.cause instanceof Error ? ` (cause: ${error.cause.message})` : "";
        console.error(`request failed: ${error instanceof Error ? error.message.split("\n")[0] : String(error)}${cause}`);
      }
    },
    confirmOrder: (input: OrderInput) => confirmOrder(input.orderId),
    relay: () => relayOutbox(),
    confirmAndRelay: async (input: OrderInput) => {
      await confirmOrder(input.orderId);
      await relayOutbox();
    },
    // The control run: every flow once, without failures.
    allFlows: async () => {
      await completeOrder("order-1");
      await recordPayment("order-2", 4200);
      await shipOrder("order-3", "TRACK-NEW");
      await confirmOrder("order-4");
      await relayOutbox();
    },
  },
  () => pool.end(),
);
