// Building a query does not execute it.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function prepare(id: string) {
  db.update(orders).set({ status: "draft" }); // built, never executed
  const statement = db.insert(orders).values({ id }).toSQL(); // SQL text only
  const prepared = db.delete(orders).prepare("delete_order"); // prepared, not executed here
  await queue.publish("order.prepared", { id, statement });
  return prepared;
}

// A non-async function returns the builder to its caller without executing it.
export const markPaidQuery = (id: string) => db.update(orders).set({ status: "paid", id });
