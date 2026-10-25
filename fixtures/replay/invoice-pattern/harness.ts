import { runScenarios } from "../../../runtime/typescript/commitrift-replay.ts";
import { db } from "../shared/db.ts";
import { finalizeInvoice } from "./src/routers/invoices.ts";

await runScenarios(
  {
    finalizeInvoice: (input: { id: string; deliveryType: string }) => finalizeInvoice(input),
  },
  () => db.close(),
);
