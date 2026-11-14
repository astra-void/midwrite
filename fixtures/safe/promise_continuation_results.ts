// DEV-172: the value of an awaited continuation chain bound to a `const`
// is what its handler returns. A handler that performs an operation and
// returns a fallback (`null`, `false`, or nothing) is followed by the
// caller's early return on that fallback, so the handler's operation is
// never followed by the code after the return. A handler that rethrows,
// or a `.then` callback that throws, ends the function.
// @expect none
import { db } from "@/db";
import { events, orders } from "@/db/schema";

export async function fallbackNull(id: string) {
  const user = await loadUser(id).catch(async () => {
    await db.insert(events).values({ id });
    return null;
  });
  if (!user) return;
  await queue.publish("user.loaded", { id });
}

export async function fallbackFalse(id: string) {
  const ok = await loadUser(id)
    .then(() => true)
    .catch(async () => {
      await queue.publish("load.failed", { id });
      return false;
    });
  if (!ok) return;
  await db.insert(orders).values({ id });
}

// Reaching the end of the handler returns `undefined`.
export async function fallbackUndefined(id: string) {
  const user = await loadUser(id).catch(async () => {
    await db.insert(events).values({ id });
  });
  if (!user) return;
  await queue.publish("user.loaded", { id });
}

export async function rethrown(id: string) {
  await loadUser(id).catch(async (error) => {
    await db.insert(events).values({ id });
    throw error;
  });
  await queue.publish("user.loaded", { id });
}

export async function thenThrows(id: string) {
  await db
    .insert(events)
    .values({ id })
    .then(() => {
      throw new Error("stop");
    });
  await queue.publish("never", { id });
}
