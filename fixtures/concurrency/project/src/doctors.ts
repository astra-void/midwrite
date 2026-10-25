import { and, count, eq, ne } from "drizzle-orm";
import { db } from "./db";
import { doctors } from "./schema";

// Each doctor may go off call only if another doctor stays on call.
export async function goOffCall(me: string) {
  await db.transaction(async (tx) => {
    const [others] = await tx
      .select({ n: count() })
      .from(doctors)
      .where(and(eq(doctors.on_call, true), ne(doctors.id, me)));
    if (others.n < 1) {
      throw new Error("No other doctor on call");
    }
    await tx.update(doctors).set({ on_call: false }).where(eq(doctors.id, me));
  });
}
