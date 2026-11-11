import { pgTable, text, uuid } from "drizzle-orm/pg-core";

// Database-side generated key: PostgreSQL computes gen_random_uuid() while
// it executes the INSERT.
export const users = pgTable("users", {
  id: uuid("id").defaultRandom().primaryKey(),
  email: text("email").notNull().unique(),
  name: text("name").notNull(),
});
