// @expect MW1001 line=4 function=notify
export async function notify() {
  await db.update(orders).set({ status: "paid" });
  await fetch("https://example.test/events", { method: "post" });
}
