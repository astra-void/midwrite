// A type-only import has no runtime value: even if code calls it, it is
// not linked to the function of the same name (the call stays unknown).
// @expect CP9001 line=11 function=save
import type { publishOrder } from "./events";
import { type OrderEvent } from "./events";
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function save(event: OrderEvent) {
  await db.update(orders).set({ status: "saved" });
  await publishOrder(event.id);
}
