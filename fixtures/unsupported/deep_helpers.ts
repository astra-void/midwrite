// Summaries are inlined at most three calls deep. `completeOrder` reaches the
// publish only through four calls, so the outer call stays an unknown effect
// and is reported as CP9001 instead of being guessed.
// @expect CP9001 line=16 function=completeOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function level4(id: string) { await queue.publish("order.completed", { id }); }
async function level3(id: string) { await level4(id); }
async function level2(id: string) { await level3(id); }
async function level1(id: string) { await level2(id); }

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  // The publish is four calls away.
  await level1(id);
}
