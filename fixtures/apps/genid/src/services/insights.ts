import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { insights } from "../schema.js";

type InsightJob = {
  data: {
    teamId: string;
    periodType: "weekly" | "monthly";
    periodYear: number;
    periodNumber: number;
  };
};

// The shape of midday's insight-generation job (look up the period's
// insight, skip a completed one, otherwise create it with ON CONFLICT DO
// NOTHING and fail when nothing was created): what the Phase 16 contract
// templates for midday's insights write skew are validated against.
export async function generateTeamInsights(job: InsightJob) {
  const { teamId, periodType, periodYear, periodNumber } = job.data;
  const [existing] = await db
    .select({ id: insights.id, status: insights.status })
    .from(insights)
    .where(
      and(
        eq(insights.teamId, teamId),
        eq(insights.periodType, periodType),
        eq(insights.periodYear, periodYear),
        eq(insights.periodNumber, periodNumber),
      ),
    )
    .limit(1);
  if (existing) {
    return { insightId: existing.id, status: "skipped" };
  }
  const [created] = await db
    .insert(insights)
    .values({ teamId, periodType, periodYear, periodNumber, status: "pending" })
    .onConflictDoNothing()
    .returning({ id: insights.id });
  if (!created) {
    throw new Error("Failed to create insight record");
  }
  return { insightId: created.id, status: "generated" };
}
