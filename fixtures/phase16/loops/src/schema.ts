import { integer, pgTable, text } from "drizzle-orm/pg-core";

export const counters = pgTable("counters", {
  id: integer("id").primaryKey(),
  value: integer("value").notNull(),
  label: text("label").notNull(),
});
