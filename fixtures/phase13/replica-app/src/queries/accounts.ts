// Query functions in the style of a production code base: the database is a
// parameter typed `Database = Awaited<ReturnType<typeof connectDb>>`, a
// replicated database built by the project's own `withReplicas`.
import { eq } from "drizzle-orm";
import {
  connectDb,
  type Database,
  type DatabaseOrTransaction,
  type DatabaseWithPrimary,
} from "../db/client.js";
import { accounts, transfers } from "../db/schema.js";

type OpenParams = { id: number; email: string };

/** Write, then read the row back: the read goes to a replica, which may not
 * have replayed the write yet. */
export async function openAccount(db: Database, params: OpenParams) {
  await db.insert(accounts).values({ id: params.id, email: params.email, balance: 0 });
  const [row] = await db
    .select()
    .from(accounts)
    .where(eq(accounts.id, params.id))
    .limit(1);
  if (!row) {
    throw new Error("account not visible after insert");
  }
  return row;
}

/** The same, reading from the primary explicitly. */
export async function openAccountOnPrimary(db: Database, params: OpenParams) {
  await db.insert(accounts).values({ id: params.id, email: params.email, balance: 0 });
  const [row] = await db
    .usePrimaryOnly()
    .select()
    .from(accounts)
    .where(eq(accounts.id, params.id))
    .limit(1);
  if (!row) {
    throw new Error("account not visible after insert");
  }
  return row;
}

/** `$primary` through an intersection type. */
export async function balanceOnPrimary(db: DatabaseWithPrimary, id: number) {
  const primary = db.$primary;
  const [row] = await primary
    .select()
    .from(accounts)
    .where(eq(accounts.id, id))
    .limit(1);
  return row;
}

/** A relational query: read from a replica. */
export async function findAccount(db: Database, id: number) {
  const row = await db.query.accounts.findFirst({ where: eq(accounts.id, id) });
  return row;
}

/** Works with a database or a transaction: bound only when inlined. */
export async function debit(db: DatabaseOrTransaction, id: number, amount: number) {
  await db
    .update(accounts)
    .set({ balance: amount })
    .where(eq(accounts.id, id));
}

/** A transaction through a replicated database runs on the primary. */
export async function transfer(db: Database, fromId: number, amount: number) {
  await db.transaction(async (tx) => {
    const [from] = await tx
      .select()
      .from(accounts)
      .where(eq(accounts.id, fromId))
      .limit(1)
      .for("update");
    if (!from) {
      throw new Error("no such account");
    }
    await debit(tx, fromId, from.balance - amount);
    await tx.insert(transfers).values({ id: fromId, fromId, amount });
  });
}

/** The helper inlined with the replicated database itself: autocommit on
 * the primary. */
export async function debitDirect(db: Database, id: number, amount: number) {
  await debit(db, id, amount);
}

/** A handle obtained from the factory function's result. */
export async function recordTransfer(fromId: number, amount: number) {
  const db = await connectDb();
  await db.insert(transfers).values({ id: fromId, fromId, amount });
  const [row] = await db
    .select()
    .from(transfers)
    .where(eq(transfers.id, fromId))
    .limit(1);
  return row;
}
