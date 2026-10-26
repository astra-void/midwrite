import { integer, pgTable, serial, text } from "drizzle-orm/pg-core";

export const orders = pgTable("orders", {
  id: integer("id").primaryKey(),
  status: text("status").notNull().default("unpaid"),
});

// One row per recorded payment. Nothing prevents two rows for the same
// logical payment (the blind and look-then-insert ledgers rely on the
// application alone).
export const payments = pgTable("payments", {
  id: serial("id").primaryKey(),
  requestId: text("request_id").notNull(),
  orderId: integer("order_id").notNull(),
  amount: integer("amount").notNull(),
});

// The idempotent ledger: the database enforces one row per request id.
export const ledger = pgTable("ledger", {
  id: serial("id").primaryKey(),
  requestId: text("request_id").notNull().unique(),
  orderId: integer("order_id").notNull(),
  amount: integer("amount").notNull(),
});

export type Payment = typeof payments.$inferSelect;
