// A Drizzle-style schema. Midwrite's model checker does NOT infer
// constraints from this file: every table, column and constraint it
// assumes is declared again in the scenario (see ../scenarios/*.json).
import { boolean, integer, pgTable, text } from "drizzle-orm/pg-core";

export const products = pgTable("products", {
  id: text("id").primaryKey(),
  stock: integer("stock").notNull(),
});

export const orders = pgTable("orders", {
  id: text("id").primaryKey(),
  product: text("product"),
});

export const doctors = pgTable("doctors", {
  id: text("id").primaryKey(),
  on_call: boolean("on_call").notNull(),
});

export const redemptions = pgTable("redemptions", {
  id: text("id").primaryKey(),
  code: text("code"),
});

export const jobs = pgTable("jobs", {
  id: text("id").primaryKey(),
  status: text("status").notNull(),
});

export const payments = pgTable("payments", {
  id: text("id").primaryKey(),
  request: text("request"),
  amount: integer("amount"),
});
