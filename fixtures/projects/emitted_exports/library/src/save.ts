// @expect none
export async function save(id: string) { await db.insert(items).values({ id }); }
