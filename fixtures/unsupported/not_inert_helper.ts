// Phase 5: a helper without recognized operations is resolved but not
// proven inert: it returns the result of a package call (adopted by the
// async function) and passes a callback to an unknown scheduler. Both calls
// stay unknown after the commit; the details say why.
// @expect CP9001 line=20
import { db } from "@/db";
import { orders } from "@/db/schema";
import { render as renderEmail } from "@react-email/render";

async function render(component: unknown) {
  return renderEmail(component);
}

async function schedule(id: string) {
  await scheduler.run(async () => notify(id));
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await render(id);
  await schedule(id);
}
