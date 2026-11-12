// @expect MW1004 line=4 function=replace
export async function replace(db: any) {
  await db.delete(oldItems);
  await db.insert(newItems).values({});
}
