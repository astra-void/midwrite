import { eq } from "drizzle-orm";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import type { PgWithReplicas } from "drizzle-orm/pg-core";
import { accounts } from "../db/schema.js";
import * as schema from "../db/schema.js";
import { db } from "./db.js";

/** Check, then insert: the check reads a replica, the insert writes the
 * primary. */
export async function register(id: number, email: string) {
  const [existing] = await db
    .select()
    .from(accounts)
    .where(eq(accounts.email, email))
    .limit(1);
  if (existing) {
    return existing.id;
  }
  await db.insert(accounts).values({ id, email, balance: 0 });
  return id;
}

/** `$primary` of drizzle-orm's replicated database. */
export async function registerOnPrimary(id: number, email: string) {
  const [existing] = await db.$primary
    .select()
    .from(accounts)
    .where(eq(accounts.email, email))
    .limit(1);
  if (existing) {
    return existing.id;
  }
  await db.insert(accounts).values({ id, email, balance: 0 });
  return id;
}

/** A row lock read through the replicated database goes to a replica. */
export async function lockOnReplica(id: number) {
  const [row] = await db
    .select()
    .from(accounts)
    .where(eq(accounts.id, id))
    .limit(1)
    .for("update");
  return row;
}

/** A parameter typed with drizzle-orm's `PgWithReplicas`. */
export async function typedReplicated(
  handle: PgWithReplicas<NodePgDatabase<typeof schema>>,
  id: number,
) {
  await handle.update(accounts).set({ balance: 1 }).where(eq(accounts.id, id));
  const [row] = await handle.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}

/** A parameter typed as a plain drizzle database in a project that builds a
 * replicated one: it may hold either. */
export async function typedPlain(handle: NodePgDatabase<typeof schema>, id: number) {
  await handle.update(accounts).set({ balance: 2 }).where(eq(accounts.id, id));
  const [row] = await handle.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}
