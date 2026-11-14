// DEV-181: the same helpers called with a transaction handle write inside
// the caller's transaction, directly or through a wrapper: nothing
// commits on its own, so nothing is reported at the caller.
// @expect none
import { db } from "@/db";
import { orders, lines } from "@/db/schema";

async function complete(store: typeof db, id: string) {
  await store.update(orders).set({ status: "paid" });
  await store.insert(lines).values({ id });
}

async function wrapper(handle: typeof db, id: string) {
  await complete(handle, id);
}

export async function inTransaction(id: string) {
  await db.transaction(async (tx) => {
    await complete(tx, id);
    await wrapper(tx, id);
  });
}
