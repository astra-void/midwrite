import { integer, pgTable, text, uuid } from "drizzle-orm/pg-core";

export const accounts = pgTable("accounts", {
  id: uuid("id").defaultRandom().primaryKey(),
  handle: text("handle").notNull().unique(),
});

export const wallets = pgTable("wallets", {
  accountId: uuid("account_id")
    .primaryKey()
    .references(() => accounts.id),
  balance: integer("balance").notNull(),
});

export const events = pgTable("events", {
  id: uuid("id").defaultRandom().primaryKey(),
  accountId: uuid("account_id").notNull(),
  kind: text("kind").notNull(),
});
