// Drizzle table definitions. The authoritative DDL (constraints included)
// is schema.sql, applied by the replay supervisor to every fresh database.
import { integer, jsonb, pgTable, serial, text, timestamp } from "drizzle-orm/pg-core";

export const orders = pgTable("orders", {
  id: text("id").primaryKey(),
  status: text("status").notNull(),
});

export const payments = pgTable("payments", {
  id: serial("id").primaryKey(),
  orderId: text("order_id").notNull(),
  amountCents: integer("amount_cents").notNull(),
});

export const shipments = pgTable("shipments", {
  orderId: text("order_id").primaryKey(),
  trackingNumber: text("tracking_number").notNull(),
});

export const outbox = pgTable("outbox", {
  id: text("id").primaryKey(),
  topic: text("topic").notNull(),
  payload: jsonb("payload").notNull(),
  publishedAt: timestamp("published_at", { withTimezone: true }),
});
