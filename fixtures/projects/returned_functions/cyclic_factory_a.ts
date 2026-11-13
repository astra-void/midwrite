import { other } from "./cyclic_factory_b";
import { db } from "@/db";
import { orders } from "@/db/schema";

function make(unused: unknown) {
  return async () => { await db.insert(orders).values({}); };
}
export const cyclic = make(other);
export async function invokeCycle() {
  await cyclic();
  await queue.publish({});
}
// @expect none
