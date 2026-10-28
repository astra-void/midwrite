// A `withReplicas` from another package: its routing is unknown, so the
// database it returns is neither the primary nor a replicated database.
import { eq } from "drizzle-orm";
import { drizzle } from "drizzle-orm/node-postgres";
import { withReplicas } from "replica-lookalike";
import { accounts } from "../db/schema.js";

const primary = drizzle(process.env.DATABASE_PRIMARY_URL!);

const db = withReplicas(primary, [primary]);

export async function lookalike(id: number) {
  await db.insert(accounts).values({ id, email: "x", balance: 0 });
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}
