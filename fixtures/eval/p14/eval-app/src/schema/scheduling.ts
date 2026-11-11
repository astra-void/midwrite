import { integer, pgTable, serial, text, timestamp, uuid } from "drizzle-orm/pg-core";

// Bookable time slots; reserved_by is the reserving account (plain column).
export const slots = pgTable("slots", {
  id: serial("id").primaryKey(),
  startsAt: timestamp("starts_at", { withTimezone: true }).notNull(),
  reservedBy: uuid("reserved_by"),
});

// Work shifts; worker_id is the worker who claimed the shift.
export const shifts = pgTable("shifts", {
  id: serial("id").primaryKey(),
  startsAt: timestamp("starts_at", { withTimezone: true }).notNull(),
  workerId: text("worker_id"),
});

// Background jobs with a time-limited lease (locked_until).
export const jobs = pgTable("jobs", {
  id: serial("id").primaryKey(),
  kind: text("kind").notNull(),
  lockedBy: text("locked_by"),
  lockedUntil: timestamp("locked_until", { withTimezone: true }),
  doneAt: timestamp("done_at", { withTimezone: true }),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  updatedAt: timestamp("updated_at", { withTimezone: true })
    .notNull()
    .defaultNow()
    .$onUpdate(() => new Date()),
});

// One row per started run of a job.
export const jobRuns = pgTable("job_runs", {
  id: serial("id").primaryKey(),
  jobId: integer("job_id")
    .notNull()
    .references(() => jobs.id),
  workerId: text("worker_id").notNull(),
  startedAt: timestamp("started_at", { withTimezone: true }).notNull(),
});
