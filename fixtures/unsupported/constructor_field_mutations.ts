// DEV-170: fields whose content the constructor alone does not decide are
// not bound to the constructor's arguments. A parameter property that
// another method reassigns, a public one (code outside the class may
// assign it), a method inherited by a subclass with its own constructor,
// and an argument that is not a recognized handle. None of the writes is
// attributed to the handle passed at construction; the unconfigured field
// names stay unanalyzed, as before.
// @expect none
import { db } from "@/db";
import { orders, lines } from "@/db/schema";

type Database = typeof db;

class Swappable {
  constructor(private store: Database) {}
  use(other: Database) {
    this.store = other;
  }
  async save(id: string) {
    await this.store.insert(lines).values({ id });
  }
}

class Open {
  constructor(public store: Database) {}
  async save(id: string) {
    await this.store.insert(lines).values({ id });
  }
}

class Base {
  constructor(private readonly store: Database) {}
  async markPaid(id: string) {
    await this.store.update(orders).set({ status: "paid" });
  }
}

class Derived extends Base {
  constructor(name: string, store: Database) {
    super(store);
  }
}

export async function reassigned(id: string) {
  await new Swappable(db).save(id);
  await queue.publish("x", { id });
}

export async function publicField(id: string) {
  await new Open(db).save(id);
  await queue.publish("x", { id });
}

export async function inherited(id: string) {
  await new Derived("a", db).markPaid(id);
  await queue.publish("x", { id });
}

export async function unknownHandle(id: string, handle: unknown) {
  await new Base(handle as Database).markPaid(id);
  await queue.publish("x", { id });
}
