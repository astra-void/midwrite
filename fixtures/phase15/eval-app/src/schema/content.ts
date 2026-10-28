import { integer, pgTable, serial, text, unique, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

export const spaces = pgTable("spaces", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  name: text("name").notNull(),
});

export const folders = pgTable("folders", {
  id: serial("id").primaryKey(),
  spaceId: integer("space_id")
    .notNull()
    .references(() => spaces.id),
  name: text("name").notNull(),
});

// Documents live in a folder, or at the root of the space (folder_id NULL).
// UNIQUE(space_id, folder_id, title) with PostgreSQL's default NULLS
// DISTINCT: two root documents never conflict.
export const documents = pgTable(
  "documents",
  {
    id: serial("id").primaryKey(),
    spaceId: integer("space_id")
      .notNull()
      .references(() => spaces.id),
    folderId: integer("folder_id").references(() => folders.id),
    title: text("title").notNull(),
  },
  (t) => [unique("documents_space_folder_title").on(t.spaceId, t.folderId, t.title)],
);

// Pages: the same shape, with UNIQUE ... NULLS NOT DISTINCT (PostgreSQL 15+):
// two root pages with the same title conflict.
export const pages = pgTable(
  "pages",
  {
    id: serial("id").primaryKey(),
    spaceId: integer("space_id")
      .notNull()
      .references(() => spaces.id),
    folderId: integer("folder_id").references(() => folders.id),
    title: text("title").notNull(),
  },
  (t) => [unique("pages_space_folder_title").on(t.spaceId, t.folderId, t.title).nullsNotDistinct()],
);

export const counters = pgTable("counters", {
  id: serial("id").primaryKey(),
  views: integer("views").notNull().default(0),
  downloads: integer("downloads").notNull().default(0),
});
