import { eq, sql } from "drizzle-orm";

import { db, type Database } from "../db.js";
import * as schema from "../schema/index.js";
import { automations } from "../schema/index.js";

type AutomationContext = {
  db: Database;
  tables: typeof schema;
  ops: { eq: typeof eq; sql: typeof sql };
};
type AutomationBody = (ctx: AutomationContext, payload: Record<string, unknown>) => Promise<unknown>;

// P16-E39: runs a workspace's automation: its JavaScript source, stored in
// the database, is compiled with the Function constructor and given the
// database handle.
export async function runAutomation(automationId: number, payload: Record<string, unknown>) {
  const [automation] = await db
    .select({ source: automations.source, enabled: automations.enabled })
    .from(automations)
    .where(eq(automations.id, automationId))
    .limit(1);
  if (!automation || !automation.enabled) return { ran: false };

  const body = new Function(
    "ctx",
    "payload",
    `"use strict"; return (async () => { ${automation.source} })();`,
  ) as AutomationBody;
  const result = await body({ db, tables: schema, ops: { eq, sql } }, payload);

  await db
    .update(automations)
    .set({ runs: sql`${automations.runs} + 1` })
    .where(eq(automations.id, automationId));
  return { ran: true, result };
}
