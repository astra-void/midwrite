// Replay entry point: runs the scenario the supervisor selects.
import { runScenarios } from "../../../runtime/typescript/commitrift-replay.ts";
import { db } from "../shared/db.ts";
import { completeOrder, completeOrderWithCompensation } from "./src/orders.ts";

await runScenarios(
  {
    completeOrder: (input: { orderId: string }) => completeOrder(input.orderId),
    completeOrderWithCompensation: (input: { orderId: string }) =>
      completeOrderWithCompensation(input.orderId),
  },
  () => db.close(),
);
