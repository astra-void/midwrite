import { boolean, integer, pgTable, serial, text, uniqueIndex } from "drizzle-orm/pg-core";

export const organizations = pgTable("organizations", {
  id: integer("id").primaryKey(),
  name: text("name").notNull(),
});

export const members = pgTable("members", {
  id: integer("id").primaryKey(),
  orgId: integer("org_id")
    .notNull()
    .references(() => organizations.id),
  email: text("email").notNull().unique(),
  role: text("role").notNull().default("member"),
});

// Pending invitations. Nothing in the schema makes (org, email) unique: the
// action's own check is the only protection.
export const invitations = pgTable("invitations", {
  id: serial("id").primaryKey(),
  orgId: integer("org_id")
    .notNull()
    .references(() => organizations.id),
  email: text("email").notNull(),
  role: text("role").notNull(),
  invitedBy: integer("invited_by")
    .notNull()
    .references(() => members.id),
  status: text("status").notNull().default("pending"),
});

// Seat invitations: the same check, backed by a composite unique index.
export const seatInvites = pgTable(
  "seat_invites",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => organizations.id),
    email: text("email").notNull(),
    status: text("status").notNull().default("pending"),
  },
  (table) => [uniqueIndex("seat_invites_org_email").on(table.orgId, table.email)],
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

export const orders = pgTable("orders", {
  id: integer("id").primaryKey(),
  customerEmail: text("customer_email").notNull(),
  amount: integer("amount").notNull(),
  status: text("status").notNull().default("unpaid"),
});

// Card charges as the application records them.
export const charges = pgTable("charges", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  amount: integer("amount").notNull(),
});

// Payments recorded from the bank feed: one row per transfer.
export const payments = pgTable("payments", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  reference: text("reference").notNull(),
  amount: integer("amount").notNull(),
});

// The same ledger with the transfer reference as a unique key.
export const settledPayments = pgTable("settled_payments", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  reference: text("reference").notNull().unique(),
  amount: integer("amount").notNull(),
});

export const refundJobs = pgTable("refund_jobs", {
  id: integer("id").primaryKey(),
  orderId: integer("order_id")
    .notNull()
    .references(() => orders.id),
  amount: integer("amount").notNull(),
  status: text("status").notNull().default("queued"),
});

// Effects a job already performed; the unique index makes a claim atomic.
export const jobEffects = pgTable(
  "job_effects",
  {
    id: serial("id").primaryKey(),
    jobId: integer("job_id").notNull(),
    effect: text("effect").notNull(),
  },
  (table) => [uniqueIndex("job_effects_job_effect").on(table.jobId, table.effect)],
);

export const tasks = pgTable("tasks", {
  id: integer("id").primaryKey(),
  title: text("title").notNull(),
  status: text("status").notNull().default("open"),
});

export const taskAssignments = pgTable("task_assignments", {
  id: serial("id").primaryKey(),
  taskId: integer("task_id")
    .notNull()
    .references(() => tasks.id),
  assignee: text("assignee").notNull(),
});

// Store managers on duty.
export const shifts = pgTable("shifts", {
  id: integer("id").primaryKey(),
  storeId: integer("store_id").notNull(),
  managerId: integer("manager_id").notNull(),
  onDuty: boolean("on_duty").notNull().default(true),
});

// Warehouse bins (stock moved between bins).
export const bins = pgTable("bins", {
  id: integer("id").primaryKey(),
  quantity: integer("quantity").notNull(),
});

export type Member = typeof members.$inferSelect;
