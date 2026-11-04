// `notify` is shadowed by a parameter: the call is not the imported function.
// It is still an unknown call after a commit.
// @expect MW9001 line=10 function=save
import { db } from "@/db";
import { orders } from "@/db/schema";
import { notify } from "./effects";

export async function save(id: string, notify: (id: string) => Promise<void>) {
  await db.update(orders).set({ status: "saved" });
  await notify(id);
}
