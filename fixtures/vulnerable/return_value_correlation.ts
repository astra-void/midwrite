// Phase 5: a helper's returned object literals become facts about the
// caller's `const` binding of the result. The publication only runs when
// `result.ok` is truthy, which only the path that committed returns; and
// the two tests of `check.ok` below cannot both pass, so `exclusive` has no
// path with both the update and the publication (Phase 4 reported CP1001).
// @expect CP1001 line=21 confidence=medium function=payOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function saveOrder(id: string) {
  const rows = await db.update(orders).set({ status: "paid" }).returning();
  if (rows.length === 0) {
    return { ok: false };
  }
  return { ok: true, id };
}

export async function payOrder(id: string) {
  const result = await saveOrder(id);
  if (result.ok) {
    await queue.publish("order.paid", { id });
  }
}

async function inspect(id: string) {
  if (id.length > 3) {
    return { ok: true };
  }
  return { ok: false };
}

export async function exclusive(id: string) {
  const check = await inspect(id);
  if (check.ok) {
    await db.update(orders).set({ status: "checked" });
  }
  if (!check.ok) {
    await queue.publish("order.rejected", { id });
  }
}
