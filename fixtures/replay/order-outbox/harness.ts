// Replay entry point: runs the scenario the supervisor selects.
import { runScenarios } from "../../../runtime/typescript/commitrift-replay.ts";
import { db } from "../shared/db.ts";
import { completeOrder } from "./src/orders.ts";
import { relayOutbox } from "./src/relay.ts";

await runScenarios(
  {
    completeOrder: (input: { orderId: string }) => completeOrder(input.orderId),
    relay: () => relayOutbox(),
    completeAndRelay: async (input: { orderId: string }) => {
      await completeOrder(input.orderId);
      await relayOutbox();
    },
  },
  () => db.close(),
);
