import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { orgs, seats } from "../schema/index.js";

// P15-E14: gives an account a seat of the organization, holding the
// organization row FOR UPDATE.
export async function addSeat(orgId: number, accountId: string) {
  return db.transaction(async (tx) => {
    const [org] = await tx
      .select({ vacancies: orgs.vacancies })
      .from(orgs)
      .where(eq(orgs.id, orgId))
      .limit(1)
      .for("update");
    if (!org) throw new Error("no organization");
    const allowed = org.vacancies >= 0;
    if (!allowed) throw new Error("no vacancies");
    await tx
      .update(orgs)
      .set({ vacancies: sql`${orgs.vacancies} - 1` })
      .where(eq(orgs.id, orgId));
    const [seat] = await tx.insert(seats).values({ orgId, accountId }).returning({ id: seats.id });
    return seat?.id;
  });
}

// P15-E15: the same with a strict comparison.
export async function addSeatChecked(orgId: number, accountId: string) {
  return db.transaction(async (tx) => {
    const [org] = await tx
      .select({ vacancies: orgs.vacancies })
      .from(orgs)
      .where(eq(orgs.id, orgId))
      .limit(1)
      .for("update");
    if (!org) throw new Error("no organization");
    const allowed = org.vacancies > 0;
    if (!allowed) throw new Error("no vacancies");
    await tx
      .update(orgs)
      .set({ vacancies: sql`${orgs.vacancies} - 1` })
      .where(eq(orgs.id, orgId));
    const [seat] = await tx.insert(seats).values({ orgId, accountId }).returning({ id: seats.id });
    return seat?.id;
  });
}
