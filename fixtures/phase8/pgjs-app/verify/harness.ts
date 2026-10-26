// AUTHORIZED SOURCE-VERIFICATION HARNESS for the postgres-js application.
//
// The application creates its postgres-js client at module load from
// POSTGRES_URL (lib/db/drizzle.ts), like saas-starter. The harness provides
// what the deployment provides — the URL (here: the run's disposable
// database) and the payment provider (a coordinator-hosted TEST DOUBLE) —
// and coordinates the installed postgres-js module before the application
// loads. Application modules are imported unchanged.
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
} from "@commitrift/replay/source";

import type { PayRequest } from "../src/app/checkout.js";
import { runWithSession } from "../src/lib/auth/session.js";
import { useMessaging } from "../src/lib/messaging/bus.js";
import { usePaymentProvider } from "../src/lib/payments/provider.js";
import type { ReserveRequest } from "../src/app/inventory.js";

const require = createRequire(import.meta.url);
const packageJson = join(dirname(require.resolve("postgres")), "..", "..", "package.json");
const version = JSON.parse(readFileSync(packageJson, "utf8")).version as string;

instrumentDrizzle(drizzle, pgCore);
coordinatePostgresJs(postgres, version);
process.env.POSTGRES_URL = sourcePostgresUrl();
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

const { client } = await import("../src/lib/db/drizzle.js");
const { acceptInvitation, inviteTeamMember } = await import("../src/app/actions.js");
const checkout = await import("../src/app/checkout.js");
const inventory = await import("../src/app/inventory.js");

type Input = { userId: number; form: Record<string, string> };

function formData(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [key, value] of Object.entries(fields)) data.set(key, value);
  return data;
}

await runSourceActor(
  {
    inviteTeamMember: (input: Input) =>
      runWithSession({ userId: input.userId }, () => inviteTeamMember({}, formData(input.form))),
    acceptInvitation: (input: Input) =>
      runWithSession({ userId: input.userId }, () => acceptInvitation({}, formData(input.form))),
    payOrder: (input: PayRequest) => checkout.payOrder(input),
    reserveStock: (input: ReserveRequest) => inventory.reserveStock(input),
  },
  () => client.end(),
);
