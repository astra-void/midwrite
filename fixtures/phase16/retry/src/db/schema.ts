import { integer, pgTable, serial, text } from "drizzle-orm/pg-core";

// Orders placed by checkout. `request_id` names the client's request; it is
// not unique.
export const orders = pgTable("orders", {
  id: serial("id").primaryKey(),
  requestId: text("request_id").notNull(),
  customerId: integer("customer_id").notNull(),
  amount: integer("amount").notNull(),
});

// Payments, keyed by the client's idempotency key (UNIQUE).
export const payments = pgTable("payments", {
  id: serial("id").primaryKey(),
  idempotencyKey: text("idempotency_key").notNull().unique(),
  orderId: integer("order_id").notNull(),
  amount: integer("amount").notNull(),
});

// Prepaid accounts.
export const accounts = pgTable("accounts", {
  id: integer("id").primaryKey(),
  balance: integer("balance").notNull(),
});
