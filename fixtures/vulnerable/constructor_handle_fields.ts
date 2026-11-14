// DEV-170: a field the constructor sets from its parameter
// (`constructor(private readonly database: Database)`, or a sealed field
// assigned once from a parameter) holds whatever handle the instance was
// constructed with. Called on `new Repo(db)`, the repository's writes are
// root commits: one before a publication, two in one method, and two
// reached through `this.m()`. The field name is not a configured receiver,
// so nothing is matched by the name `database` alone.
// @expect MW1001 line=49 confidence=high function=payAndPublish
// @expect MW1004 line=53 confidence=high function=completeOrder
// @expect MW1004 line=57 confidence=high function=viaThis
import { db } from "@/db";
import { orders, lines } from "@/db/schema";

type Database = typeof db;

class Repo {
  constructor(private readonly database: Database) {}

  async markPaid(id: string) {
    await this.database.update(orders).set({ status: "paid" });
  }

  async complete(id: string) {
    await this.database.update(orders).set({ status: "paid" });
    await this.database.insert(lines).values({ id });
  }
}

class Ledger {
  private readonly store: Database;
  constructor(store: Database) {
    this.store = store;
  }
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

export async function payAndPublish(id: string) {
  const repo = new Repo(db);
  await repo.markPaid(id);
  await queue.publish("order.paid", { id });
}

export async function completeOrder(id: string) {
  await new Repo(db).complete(id);
}

export async function viaThis(id: string) {
  await new Ledger(db).complete(id);
}
