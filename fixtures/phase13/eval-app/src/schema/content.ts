import { integer, pgTable, serial, text, unique, uuid } from "drizzle-orm/pg-core";

import { users as accounts } from "./users.js";

export const posts = pgTable("posts", {
  id: serial("id").primaryKey(),
  authorId: uuid("author_id")
    .notNull()
    .references(() => accounts.id),
  title: text("title").notNull(),
});

export const comments = pgTable("comments", {
  id: serial("id").primaryKey(),
  postId: integer("post_id")
    .notNull()
    .references(() => posts.id),
  authorId: uuid("author_id").notNull(),
  body: text("body").notNull(),
});

// Bookmarks keep the post id as a plain column (no foreign key).
export const bookmarks = pgTable("bookmarks", {
  id: serial("id").primaryKey(),
  postId: integer("post_id").notNull(),
  userId: uuid("user_id").notNull(),
});

export const pageCounters = pgTable("page_counters", {
  path: text("path").primaryKey(),
  hits: integer("hits").notNull(),
});

// A folder at the root has no parent (parent_id IS NULL).
export const folders = pgTable(
  "folders",
  {
    id: serial("id").primaryKey(),
    ownerId: uuid("owner_id")
      .notNull()
      .references(() => accounts.id),
    parentId: integer("parent_id"),
    name: text("name").notNull(),
  },
  (t) => [unique("folders_owner_parent_name").on(t.ownerId, t.parentId, t.name)],
);
