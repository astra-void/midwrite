// `getDb()` is not a recognized handle, so the helper's write is not guessed
// to be on any particular handle: the call stays unknown (CP9001 after the
// commit), and no CP1004 is claimed.
// @expect CP9001 line=11 function=save
import { db } from "@/db";
import { audit } from "@/db/schema";
import { saveOrder } from "./repository";

export async function save(id: string) {
  await db.insert(audit).values({ id });
  await saveOrder(getDb(), id);
}
