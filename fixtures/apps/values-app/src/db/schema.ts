import { integer, pgTable, text } from "drizzle-orm/pg-core";

// A post's tags are stored as one comma-separated string.
export const posts = pgTable("posts", {
  id: integer("id").primaryKey(),
  tags: text("tags").notNull(),
});

// Members of a team, by normalized email. No unique constraint: the
// application checks before inserting.
export const members = pgTable("members", {
  id: integer("id").primaryKey(),
  teamId: integer("team_id").notNull(),
  email: text("email").notNull(),
  role: text("role").notNull(),
});

// Reserved handles (`first.last`, lower case). No unique constraint.
export const handles = pgTable("handles", {
  id: integer("id").primaryKey(),
  handle: text("handle").notNull(),
});

export const audit = pgTable("audit", {
  id: integer("id").primaryKey(),
  note: text("note"),
});
