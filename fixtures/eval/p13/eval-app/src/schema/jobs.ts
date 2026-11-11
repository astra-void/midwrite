import { integer, pgTable, text, uuid } from "drizzle-orm/pg-core";

export const notifications = pgTable("notifications", {
  id: uuid("id").defaultRandom().primaryKey(),
  userId: uuid("user_id").notNull(),
  kind: text("kind").notNull(),
  jobId: integer("job_id"),
});

export const processedJobs = pgTable("processed_jobs", {
  jobId: integer("job_id").primaryKey(),
});
