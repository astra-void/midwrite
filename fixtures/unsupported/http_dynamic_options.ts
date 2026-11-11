// @expect MW9001 line=4 function=notify
export async function notify(options: RequestInit) {
  await db.update(orders).set({ status: "paid" });
  await fetch("https://example.test/events", { method: "POST", ...options });
}
