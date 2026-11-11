import { pgTable, serial, text } from "drizzle-orm/pg-core";

import { projects } from "./projects.js";

export const tasks = pgTable("tasks", {
  id: serial("id").primaryKey(),
  projectId: text("project_id")
    .notNull()
    .references(() => projects.id),
  title: text("title").notNull(),
  assignee: text("assignee"),
});
