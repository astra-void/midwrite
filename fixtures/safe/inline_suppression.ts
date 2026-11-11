// @expect none
export async function reconciledOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  // midwrite-ignore MW1001: compensated by nightly reconciliation job
  await queue.publish("order.completed", { id });
}

export async function reconciledInvoice(id: string) {
  await db.update(invoices).set({ status: "paid" });
  await queue.publish("invoice.completed", { id }); // midwrite-ignore MW1001: reconciliation retries publication
}
