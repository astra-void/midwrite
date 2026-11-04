// The imported helper performs no recognized operation and calls nothing:
// it is linked, and since Phase 5 its effect summary proves it inert (no
// write, transaction, external effect or unknown call on any path), so the
// call is resolved instead of staying an unknown call after the commit.
// (Phase 4 reported MW9001 at line 11.)
// @expect none
import { db } from "@/db";
import { formatReceipt } from "./format";

export async function pay(id: string) {
  await db.update(orders).set({ status: "paid" });
  const receipt = await formatReceipt(id);
  return receipt;
}
