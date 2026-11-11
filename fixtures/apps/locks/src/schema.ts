import { integer, pgTable, serial, text, unique } from "drizzle-orm/pg-core";

// Accounts whose balances move between each other.
export const accounts = pgTable("accounts", {
  id: integer("id").primaryKey(),
  balance: integer("balance").notNull().default(0),
});

// Spaces own documents.
export const spaces = pgTable("spaces", {
  id: integer("id").primaryKey(),
  name: text("name").notNull(),
});

// A document lives in a folder of a space, or at the root of the space when
// folder_id IS NULL. UNIQUE(space_id, folder_id, title) with PostgreSQL's
// default NULLS DISTINCT: two root documents with one title never conflict.
export const documents = pgTable(
  "documents",
  {
    id: serial("id").primaryKey(),
    spaceId: integer("space_id").notNull(),
    folderId: integer("folder_id"),
    title: text("title").notNull(),
  },
  (t) => [unique("documents_space_folder_title").on(t.spaceId, t.folderId, t.title)],
);
