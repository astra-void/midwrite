// DEV-170: repositories constructed with a transaction handle write
// inside the caller's transaction, so nothing commits on its own.
// `this.db` is also a configured receiver; the constructor binding wins
// over the name, so a repository built on `tx` is not taken for the root
// handle. A transaction a method starts on the field is a savepoint of the
// caller's transaction.
// @expect none
import { db } from "@/db";
import { orders, lines } from "@/db/schema";

type Database = typeof db;

class Repo {
  constructor(private readonly db: Database) {}

  async markPaid(id: string) {
    await this.db.update(orders).set({ status: "paid" });
  }

  async atomically(id: string) {
    await this.db.transaction(async (tx) => {
      await tx.update(orders).set({ status: "paid" });
      await tx.insert(lines).values({ id });
    });
  }
}

class Ledger {
  constructor(private readonly store: Database) {}
  async markPaid(id: string) {
    await this.store.update(orders).set({ status: "paid" });
  }
  async addLine(id: string) {
    await this.store.insert(lines).values({ id });
  }
  async complete(id: string) {
    await this.markPaid(id);
    await this.addLine(id);
  }
}

export async function inTransaction(id: string) {
  await db.transaction(async (tx) => {
    await new Repo(tx).markPaid(id);
    await new Ledger(tx).complete(id);
  });
}

export async function nested(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "pending" });
    await new Repo(tx).atomically(id);
  });
}
