// DEV-172: feasible paths through awaited continuations stay reported.
// * A swallowed error: the function continues after the handler.
// * A best-effort write (`.catch(() => {})`): a real window before what
//   follows; whether it is accepted is a labeling question, not a rule.
// * A `return` inside a `.then` callback leaves the callback only.
// * The value of `p.finally(cb)` is `p`'s; what `cb` returns is discarded,
//   so the early return is not decided by it.
// * On the fulfilled path of `.then(() => true).catch(...)` the value is
//   `true`: the insert is followed by the publication.
// @expect MW1001 line=22 confidence=low function=swallowed
// @expect MW1001 line=27 confidence=high function=bestEffort
// @expect MW1001 line=38 function=returnInsideThen
// @expect MW1001 line=48 function=finallyKeepsTheValue
// @expect MW1001 line=58 function=fulfilledPath
import { db } from "@/db";
import { events, heartbeats, orders } from "@/db/schema";

export async function swallowed(id: string) {
  await loadUser(id).catch(async () => {
    await db.update(orders).set({ status: "failed" });
  });
  await queue.publish("continued", { id });
}

export async function bestEffort(id: string) {
  await db.update(heartbeats).set({ seen: true }).catch(() => {});
  await queue.publish("upload", { id });
}

export async function returnInsideThen(id: string, quiet: boolean) {
  await db
    .insert(events)
    .values({ id })
    .then(() => {
      if (quiet) return;
      console.log(id);
    });
  await queue.publish("after", { id });
}

export async function finallyKeepsTheValue(id: string) {
  const rows = await db
    .insert(orders)
    .values({ id })
    .returning()
    .finally(() => null);
  if (!rows) return;
  await queue.publish("order.created", { id });
}

export async function fulfilledPath(id: string) {
  const ok = await db
    .insert(orders)
    .values({ id })
    .then(() => true)
    .catch(() => false);
  if (!ok) return;
  await queue.publish("order.created", { id });
}
