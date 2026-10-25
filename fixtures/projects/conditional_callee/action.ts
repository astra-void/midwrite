// The callee publishes only on some of its paths: medium confidence.
// @expect CP1001 line=8 confidence=medium function=pay
import { db } from "@/db";
import { maybeNotify } from "./notify";

export async function pay(id: string) {
  await db.update(orders).set({ status: "paid" });
  await maybeNotify(id, true);
}
