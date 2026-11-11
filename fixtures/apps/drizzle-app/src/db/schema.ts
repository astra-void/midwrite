import {
  integer,
  jsonb,
  pgTable,
  serial,
  text,
  timestamp,
  unique,
} from "drizzle-orm/pg-core";

export const products = pgTable("products", {
  sku: text("sku").primaryKey(),
  name: text("name").notNull(),
  stock: integer("stock").notNull(),
});

export const orders = pgTable("orders", {
  id: integer("id").primaryKey(),
  customer: text("customer").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
});

// Order lines: a serial key, a foreign key to the order and to the product.
export const orderItems = pgTable("order_items", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id").notNull().references(() => orders.id),
  sku: text("sku").notNull().references(() => products.sku),
  qty: integer("qty").notNull(),
});

// Tags of a post: one row per (post, tag).
export const postTags = pgTable(
  "post_tags",
  {
    id: serial("id").primaryKey(),
    postId: integer("post_id").notNull(),
    tag: text("tag").notNull(),
  },
  (t) => [unique("post_tags_post_tag").on(t.postId, t.tag)],
);

// Comments form a tree; `parentId` is NULL for a top-level comment.
export const comments = pgTable("comments", {
  id: integer("id").primaryKey(),
  parentId: integer("parent_id"),
  body: text("body").notNull(),
  status: text("status"),
});

// Users with a nullable timestamp a job handler sets once.
export const users = pgTable("users", {
  id: integer("id").primaryKey(),
  email: text("email").notNull(),
  welcomedAt: timestamp("welcomed_at", { withTimezone: true }),
  updatedAt: timestamp("updated_at", { withTimezone: true }).$onUpdate(() => new Date()),
});

// Counters keyed by name, kept with an upsert.
export const counters = pgTable("counters", {
  name: text("name").primaryKey(),
  hits: integer("hits").notNull(),
  updatedAt: timestamp("updated_at", { withTimezone: true }),
});

// Per-user settings stored as jsonb.
export const settings = pgTable("settings", {
  userId: integer("user_id").primaryKey(),
  prefs: jsonb("prefs"),
});
