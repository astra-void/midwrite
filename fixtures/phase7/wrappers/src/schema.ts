import { integer, pgTable, serial, text } from "drizzle-orm/pg-core";

export const orders = pgTable("orders", {
  id: serial("id").primaryKey(),
  status: text("status").notNull(),
});

export const audit = pgTable("audit", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id").notNull(),
});
