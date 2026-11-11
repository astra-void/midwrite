import { index, integer, pgTable, serial, text } from "drizzle-orm/pg-core";

export type JobStatus = "queued" | "running" | "done" | "failed";
export type TaskStatus = "ready" | "claimed" | "done";

// Background jobs. A job moves queued -> running -> done | failed and is
// never re-queued (a retry is a new job).
export const jobs = pgTable("jobs", {
  id: serial("id").primaryKey(),
  kind: text("kind").notNull(),
  status: text("status").$type<JobStatus>().notNull().default("queued"),
  workerId: text("worker_id"),
});

// One row per started run; job_id indexed, NOT unique.
export const jobRuns = pgTable(
  "job_runs",
  {
    id: serial("id").primaryKey(),
    jobId: integer("job_id")
      .notNull()
      .references(() => jobs.id),
    workerId: text("worker_id").notNull(),
  },
  (t) => [index("job_runs_job").on(t.jobId)],
);

// A work queue polled in batches by several workers.
export const tasks = pgTable("tasks", {
  id: serial("id").primaryKey(),
  status: text("status").$type<TaskStatus>().notNull().default("ready"),
  payload: text("payload").notNull(),
});

// One row per claim; task_id indexed, NOT unique.
export const taskClaims = pgTable(
  "task_claims",
  {
    id: serial("id").primaryKey(),
    taskId: integer("task_id")
      .notNull()
      .references(() => tasks.id),
    workerId: text("worker_id").notNull(),
  },
  (t) => [index("task_claims_task").on(t.taskId)],
);
