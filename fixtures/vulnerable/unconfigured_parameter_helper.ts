// DEV-181: a helper whose database operations are on an unconfigured
// parameter (`store`) does not know, on its own, which handle it writes
// with; the caller passing `db` decides. A pair inside the helper is then
// reported where the helper is called, also through a wrapper that
// forwards its own parameter. A helper whose parameter has a configured
// name (`db`) is a root handle in its own analysis and reports the pair
// itself, once.
// @expect MW1004 line=35 confidence=high function=completeOrder
// @expect MW1001 line=39 confidence=high function=payAndNotify
// @expect MW1004 line=43 confidence=high function=viaWrapper
// @expect MW1004 line=31 confidence=high function=configuredHelper
import { db } from "@/db";
import { orders, lines } from "@/db/schema";

async function complete(store: typeof db, id: string) {
  await store.update(orders).set({ status: "paid" });
  await store.insert(lines).values({ id });
}

async function payThenPublish(store: typeof db, id: string) {
  await store.update(orders).set({ status: "paid" });
  await queue.publish("order.paid", { id });
}

async function wrapper(handle: typeof db, id: string) {
  await complete(handle, id);
}

async function configuredHelper(db: any, id: string) {
  await db.update(orders).set({ status: "paid" });
  await db.insert(lines).values({ id });
}

export async function completeOrder(id: string) {
  await complete(db, id);
}

export async function payAndNotify(id: string) {
  await payThenPublish(db, id);
}

export async function viaWrapper(id: string) {
  await wrapper(db, id);
}

export async function callsConfigured(id: string) {
  await configuredHelper(db, id);
}
