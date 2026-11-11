import { pgTable, text, timestamp, uuid } from "drizzle-orm/pg-core";

export const users = pgTable("users", {
  id: uuid("id").defaultRandom().primaryKey(),
  email: text("email").notNull().unique(),
  name: text("name").notNull(),
  welcomedAt: timestamp("welcomed_at"),
});

// Public handles: the handle itself is the key.
export const handles = pgTable("handles", {
  handle: text("handle").primaryKey(),
  userId: uuid("user_id")
    .notNull()
    .references(() => users.id),
  updatedAt: timestamp("updated_at").notNull().defaultNow(),
});
