import { pgTable, text, unique, uuid } from "drizzle-orm/pg-core";
import { nanoid } from "nanoid";

import { accounts as owners } from "./accounts.js";

export const projects = pgTable(
  "projects",
  {
    id: text("id")
      .$defaultFn(() => nanoid())
      .primaryKey(),
    ownerId: uuid("owner_id")
      .notNull()
      .references(() => owners.id),
    slug: text("slug").notNull(),
  },
  (t) => [unique("projects_owner_slug").on(t.ownerId, t.slug)],
);

export const members = pgTable("members", {
  id: uuid("id").defaultRandom().primaryKey(),
  projectId: text("project_id")
    .notNull()
    .references(() => projects.id),
  accountId: uuid("account_id").notNull(),
});
