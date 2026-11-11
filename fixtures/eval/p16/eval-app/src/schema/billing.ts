import { index, integer, pgTable, serial, text, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";
import { orders } from "./commerce.js";
import { workspaces } from "./workspaces.js";

export type ExpenseStatus = "submitted" | "approved" | "rejected";

export const expenses = pgTable("expenses", {
  id: serial("id").primaryKey(),
  workspaceId: integer("workspace_id")
    .notNull()
    .references(() => workspaces.id),
  submittedBy: uuid("submitted_by")
    .notNull()
    .references(() => accounts.id),
  amountCents: integer("amount_cents").notNull(),
  status: text("status").$type<ExpenseStatus>().notNull().default("submitted"),
  approvedBy: uuid("approved_by").references(() => accounts.id),
});

// expense_id is indexed, NOT unique.
export const reimbursements = pgTable(
  "reimbursements",
  {
    id: serial("id").primaryKey(),
    expenseId: integer("expense_id")
      .notNull()
      .references(() => expenses.id),
    amountCents: integer("amount_cents").notNull(),
  },
  (t) => [index("reimbursements_expense").on(t.expenseId)],
);

// Card payments recorded against an order. request_id identifies the
// client request; indexed, NOT unique.
export const payments = pgTable(
  "payments",
  {
    id: serial("id").primaryKey(),
    orderId: integer("order_id")
      .notNull()
      .references(() => orders.id),
    requestId: uuid("request_id").notNull(),
    amountCents: integer("amount_cents").notNull(),
  },
  (t) => [index("payments_request").on(t.requestId)],
);

// Payment receipts: the same shape with UNIQUE(request_id).
export const paymentReceipts = pgTable("payment_receipts", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  requestId: uuid("request_id").notNull().unique(),
  amountCents: integer("amount_cents").notNull(),
});

export const creditBalances = pgTable("credit_balances", {
  accountId: uuid("account_id")
    .primaryKey()
    .references(() => accounts.id),
  credits: integer("credits").notNull().default(0),
});

// One row per credit grant; request_id indexed, NOT unique.
export const creditEntries = pgTable(
  "credit_entries",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    requestId: uuid("request_id").notNull(),
    amount: integer("amount").notNull(),
  },
  (t) => [index("credit_entries_request").on(t.requestId)],
);

// Request ids of credit grants already applied.
export const creditRequests = pgTable("credit_requests", {
  requestId: uuid("request_id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
});

export const wallets = pgTable("wallets", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  balance: integer("balance").notNull().default(0),
});

// Transactional outbox: a relay publishes every row to the message bus.
// request_id indexed, NOT unique.
export const outboxEvents = pgTable(
  "outbox_events",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    requestId: uuid("request_id").notNull(),
    topic: text("topic").notNull(),
    walletId: integer("wallet_id")
      .notNull()
      .references(() => wallets.id),
    amount: integer("amount").notNull(),
  },
  (t) => [index("outbox_events_request").on(t.requestId)],
);

export const ledgerAccounts = pgTable("ledger_accounts", {
  id: serial("id").primaryKey(),
  ownerId: uuid("owner_id")
    .notNull()
    .references(() => accounts.id),
  balance: integer("balance").notNull(),
});

export const ledgerTransfers = pgTable("ledger_transfers", {
  id: uuid("id")
    .primaryKey()
    .$defaultFn(() => crypto.randomUUID()),
  fromId: integer("from_id")
    .notNull()
    .references(() => ledgerAccounts.id),
  toId: integer("to_id")
    .notNull()
    .references(() => ledgerAccounts.id),
  amount: integer("amount").notNull(),
});
