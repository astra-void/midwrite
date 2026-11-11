import { index, integer, pgTable, serial, text, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

export const wallets = pgTable("wallets", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  balance: integer("balance").notNull().default(0),
});

// Exports an account may still start this period: one row per account.
export const quotas = pgTable("quotas", {
  accountId: uuid("account_id")
    .primaryKey()
    .references(() => accounts.id),
  remaining: integer("remaining").notNull(),
});

export const exportJobs = pgTable("export_jobs", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  format: text("format").notNull(),
});

// Credit grants. grant_key is an optional idempotency key; the unique
// constraint is NULLS DISTINCT, so rows without a key never conflict.
export const grants = pgTable("grants", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  reason: text("reason").notNull(),
  amount: integer("amount").notNull(),
  grantKey: text("grant_key").unique("grants_grant_key"),
});

// Loyalty events already applied, by the event id of the message bus.
export const processedEvents = pgTable("processed_events", {
  eventId: text("event_id").primaryKey(),
});

// Loyalty points ledger: one row per applied event. event_id is indexed, not
// unique.
export const ledger = pgTable(
  "ledger",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    eventId: text("event_id").notNull(),
    points: integer("points").notNull(),
  },
  (t) => [index("ledger_event").on(t.eventId)],
);
