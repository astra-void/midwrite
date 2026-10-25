// A file that does not parse is reported as a failure and never analyzed.
// @expect parse-error
export async function broken(id: string) {
  await db.update(orders).set({ status: "paid" };
}
