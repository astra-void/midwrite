// Methods of a `const` object literal declared in the same file are
// summarized like functions; an object whose properties are reassigned
// anywhere in the file is not followed.
// @expect MW1001 line=20 confidence=high function=complete
// @expect MW9001 line=25 function=completeMutable
import { db } from "@/db";
import { orders } from "@/db/schema";

const repository = {
  save: async (id: string) => {
    await db.update(orders).set({ id });
  },
};

const mutable = { async save(id: string) { await db.update(orders).set({ id }); } };
mutable.save = async () => {};

export async function complete(id: string) {
  await repository.save(id);
  await queue.publish("order.completed", { id });
}

export async function completeMutable(id: string) {
  await db.insert(orders).values({ id });
  await mutable.save(id);
}
