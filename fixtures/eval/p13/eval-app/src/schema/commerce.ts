import { integer, pgTable, serial, text, uuid } from "drizzle-orm/pg-core";

import { users } from "./users.js";

export const seats = pgTable("seats", {
  id: serial("id").primaryKey(),
  eventId: integer("event_id").notNull(),
  label: text("label").notNull(),
  holder: text("holder"),
});

export const orders = pgTable("orders", {
  id: uuid("id").defaultRandom().primaryKey(),
  customerId: uuid("customer_id")
    .notNull()
    .references(() => users.id),
  cartId: text("cart_id").notNull(),
  total: integer("total").notNull(),
  status: text("status").notNull().default("pending"),
});

export const payments = pgTable("payments", {
  id: serial("id").primaryKey(),
  orderId: uuid("order_id")
    .notNull()
    .references(() => orders.id),
  amount: integer("amount").notNull(),
});
