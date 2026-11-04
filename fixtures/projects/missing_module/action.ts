// The imported module does not exist among the analyzed files: the call
// stays an unknown effect, reported because it runs after a commit.
// @expect MW9001 line=10 function=save
import { db } from "@/db";
import { orders } from "@/db/schema";
import { notify } from "./does-not-exist";

export async function save(id: string) {
  await db.update(orders).set({ status: "saved" });
  await notify(id);
}
