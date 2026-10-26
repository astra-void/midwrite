import { boolean, integer, pgTable, serial, text, timestamp, uniqueIndex } from "drizzle-orm/pg-core";

export const orders = pgTable("orders", {
  id: integer("id").primaryKey(),
  customerEmail: text("customer_email").notNull(),
  amount: integer("amount").notNull(),
  status: text("status").notNull().default("unpaid"),
  updatedAt: timestamp("updated_at").notNull().defaultNow(),
});

// One row per attempt to pay (no uniqueness: the vulnerable checkout records
// every attempt).
export const paymentAttempts = pgTable("payment_attempts", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  amount: integer("amount").notNull(),
});

// The idempotent checkout's request ledger: one row per client request id.
export const idempotencyKeys = pgTable("idempotency_keys", {
  key: text("key").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  status: text("status").notNull().default("started"),
});

// Transactional outbox: events written in the same transaction as the state
// change they describe, published later by a relay.
export const outbox = pgTable("outbox", {
  id: serial("id").primaryKey(),
  topic: text("topic").notNull(),
  orderId: integer("order_id").notNull(),
  published: boolean("published").notNull().default(false),
});

// A job queue table (claimed by workers).
export const jobs = pgTable("jobs", {
  id: integer("id").primaryKey(),
  kind: text("kind").notNull(),
  orderId: integer("order_id").notNull(),
  status: text("status").notNull().default("queued"),
});

// Effects already performed for a job (the worker's persistent idempotency
// record). The unique index makes the claim atomic.
export const jobEffects = pgTable(
  "job_effects",
  {
    id: serial("id").primaryKey(),
    jobId: integer("job_id").notNull(),
    effect: text("effect").notNull(),
  },
  (table) => [uniqueIndex("job_effects_job_effect_unique").on(table.jobId, table.effect)],
);

export const products = pgTable("products", {
  id: integer("id").primaryKey(),
  sku: text("sku").notNull().unique(),
  stock: integer("stock").notNull(),
});

export const reservations = pgTable("reservations", {
  id: serial("id").primaryKey(),
  productId: integer("product_id")
    .notNull()
    .references(() => products.id),
  quantity: integer("quantity").notNull(),
});

export type Order = typeof orders.$inferSelect;
