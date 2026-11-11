import { integer, pgTable, text } from "drizzle-orm/pg-core";

export const accounts = pgTable("accounts", {
  id: integer("id").primaryKey(),
  email: text("email").notNull(),
  balance: integer("balance").notNull(),
});

export const transfers = pgTable("transfers", {
  id: integer("id").primaryKey(),
  fromId: integer("from_id").notNull(),
  amount: integer("amount").notNull(),
});
