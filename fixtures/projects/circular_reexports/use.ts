// @expect MW9001 line=8 function=run
import { missing } from "./a";
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function run(id: string) {
  await db.update(orders).set({ status: "x" });
  await missing(id);
}
