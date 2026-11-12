// @expect MW1004 line=8 function=replace
async function lockRows(tx: any, id: string) {
  return tx.select().from(oldItems).where({ id }).for("update");
}
export async function replace(db: any, id: string) {
  await lockRows(db, id);
  await db.delete(oldItems).where({ id });
  await db.insert(newItems).values({ id });
}
