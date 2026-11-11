// @expect parse-error
export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  // midwrite-ignore MW1001:
  await queue.publish("order.completed", { id });
}
