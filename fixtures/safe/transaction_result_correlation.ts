// The audit insert runs only when the transaction returned "forbidden", which
// it does before writing: no path commits both. The literal returned by an
// inline transaction callback is a fact about the `const` holding its result.
// @expect none
import { db } from "@/db";
import { reports, audit } from "@/db/schema";

export async function withdrawReport(id: string) {
  const result = await db.transaction(async (tx) => {
    const [report] = await tx.select().from(reports).where(eq(reports.id, id));
    if (!report) return "forbidden" as const;
    await tx.update(reports).set({ status: "withdrawn" });
    return { replayed: false };
  });
  if (result === "forbidden") {
    await db.insert(audit).values({ action: "denied" });
  }
}
