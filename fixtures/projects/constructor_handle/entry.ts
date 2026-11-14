// @expect MW1001 line=8 function=handle
// @expect MW1004 line=18 function=outside
import { OrderRepository } from "./repository";

export async function handle(id: string) {
  const repository = new OrderRepository(db);
  await repository.markPaid(id);
  await queue.publish("order.paid", { id });
}

export async function inTransaction(id: string) {
  await db.transaction(async (tx) => {
    await new OrderRepository(tx).complete(id);
  });
}

export async function outside(id: string) {
  await new OrderRepository(db).complete(id);
}
