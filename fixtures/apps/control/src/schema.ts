import { integer, pgTable, text } from "drizzle-orm/pg-core";

export const orders = pgTable("orders", {
  id: integer("id").primaryKey(),
  status: text("status").notNull(),
});

export const failures = pgTable("failures", {
  orderId: integer("order_id").primaryKey(),
});
