// DEV-169: a `.then`, `.catch` or `.finally` handler that contains a
// recognized operation but is not analyzed in the function's sequence is a
// coverage gap (MW9001), reported at the handler with the reason. No
// ordering is invented: nothing pairs the handler's publication with the
// function's own commit. Several gaps in one function are one notice that
// lists every location.
//
// Controls (no notice): an awaited chain with one inline handler is
// analyzed in sequence (see vulnerable/promise_continuations.ts); handlers
// that only map or log values are not gaps; a timer callback is deferred
// code, not a continuation of the current path.
// @expect MW9001 line=23 function=discardedThen
// @expect MW9001 line=29 function=discardedCatch
// @expect MW9001 line=35 function=discardedFinally
// @expect MW9001 line=42 function=twoHandlers
// @expect MW9001 line=49 function=returnedFromSync
// @expect MW9001 line=55 function=several
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function discardedThen(id: string) {
  await db.update(orders).set({ status: "paid" });
  audit(id).then(async () => {
    await queue.publish("order.paid", { id });
  });
}

export async function discardedCatch(id: string) {
  db.update(orders).set({ status: "paid" }).catch(async () => {
    await queue.publish("order.failed", { id });
  });
}

export async function discardedFinally(id: string) {
  audit(id).finally(async () => {
    await db.update(orders).set({ status: "audited" });
  });
}

export async function twoHandlers(id: string) {
  await db.update(orders).set({ status: "paid" });
  await audit(id).then(
    () => queue.publish("order.audited", { id }),
    () => queue.publish("order.audit-failed", { id }),
  );
}

export function returnedFromSync(id: string) {
  return db.update(orders).set({ status: "paid" }).then(async () => {
    await queue.publish("order.paid", { id });
  });
}

export async function several(id: string) {
  audit(id).then(() => queue.publish("first", { id }));
  audit(id).catch(() => queue.publish("second", { id }));
}

export async function harmlessHandlers(id: string) {
  await db.update(orders).set({ status: "paid" });
  audit(id).then((result) => console.log(result));
  audit(id).catch(console.error);
}

export async function timer(id: string) {
  await db.update(orders).set({ status: "paid" });
  setTimeout(async () => {
    await queue.publish("order.reminder", { id });
  }, 1000);
}
