// @expect none
function lazy(id: string) { return db.insert(items).values({ id }); }
function relay(id: string) { return lazy(id); }
async function discard(ids: string[]) {
  await queue.publish("before", { ids });
  ids.map(id => relay(id));
}
async function noJoin(ids: string[]) {
  const tasks = ids.map(id => db.insert(items).values({ id }));
  tasks.length;
}
async function singleton() {
  await Promise.all([{}].map(async item => {
    if (condition) {
      await db.update(items).set({ active: true });
      return;
    }
    await db.insert(items).values({ active: true });
  }));
}
