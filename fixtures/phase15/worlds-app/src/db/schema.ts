import { integer, pgTable, serial, text, unique } from "drizzle-orm/pg-core";

// Folders of an owner; a root folder has no parent (parent_id IS NULL).
// UNIQUE(owner_id, parent_id, name) with PostgreSQL's default NULLS
// DISTINCT: two root folders with one name do not conflict.
export const folders = pgTable(
  "folders",
  {
    id: serial("id").primaryKey(),
    ownerId: integer("owner_id").notNull(),
    parentId: integer("parent_id"),
    name: text("name").notNull(),
  },
  (t) => [unique("folders_owner_parent_name").on(t.ownerId, t.parentId, t.name)],
);

// The same for labels, with NULLS NOT DISTINCT: one root label per name.
export const labels = pgTable(
  "labels",
  {
    id: serial("id").primaryKey(),
    ownerId: integer("owner_id").notNull(),
    parentId: integer("parent_id"),
    name: text("name").notNull(),
  },
  (t) => [unique("labels_owner_parent_name").on(t.ownerId, t.parentId, t.name).nullsNotDistinct()],
);

// Exports left to an account this period.
export const quotas = pgTable("quotas", {
  accountId: integer("account_id").primaryKey(),
  remaining: integer("remaining").notNull(),
});

export const exportJobs = pgTable("export_jobs", {
  id: serial("id").primaryKey(),
  accountId: integer("account_id").notNull(),
});
