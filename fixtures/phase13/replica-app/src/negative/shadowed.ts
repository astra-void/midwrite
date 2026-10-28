// A module-level function that shadows the name `withReplicas`, and a
// pass-through wrapper: neither is drizzle-orm's, and neither result is
// recognized (arguments are not substituted).
import { eq } from "drizzle-orm";
import { drizzle } from "drizzle-orm/node-postgres";
import { accounts } from "../db/schema.js";

const primary = drizzle(process.env.DATABASE_PRIMARY_URL!);

function withReplicas<Q>(main: Q, replicas: Q[]): Q {
  return replicas[0] ?? main;
}

function passThrough<Q>(database: Q): Q {
  return database;
}

const db = withReplicas(primary, [primary]);

const wrapped = passThrough(primary);

export async function shadowed(id: number) {
  await db.insert(accounts).values({ id, email: "x", balance: 0 });
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}

export async function viaWrapper(id: number) {
  await wrapped.insert(accounts).values({ id, email: "x", balance: 0 });
}

/** The recognized primary itself, used under another name. */
export async function onPrimary(id: number) {
  await primary.insert(accounts).values({ id, email: "x", balance: 0 });
}
