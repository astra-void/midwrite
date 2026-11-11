-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "organizations" (
	"id" integer PRIMARY KEY NOT NULL,
	"name" text NOT NULL
);
CREATE TABLE "members" (
	"id" integer PRIMARY KEY NOT NULL,
	"org_id" integer NOT NULL,
	"email" text NOT NULL,
	"role" text DEFAULT 'member' NOT NULL,
	CONSTRAINT "members_email_unique" UNIQUE("email")
);
CREATE TABLE "invitations" (
	"id" serial PRIMARY KEY NOT NULL,
	"org_id" integer NOT NULL,
	"email" text NOT NULL,
	"role" text NOT NULL,
	"invited_by" integer NOT NULL,
	"status" text DEFAULT 'pending' NOT NULL
);
CREATE TABLE "seat_invites" (
	"id" serial PRIMARY KEY NOT NULL,
	"org_id" integer NOT NULL,
	"email" text NOT NULL,
	"status" text DEFAULT 'pending' NOT NULL
);
CREATE TABLE "products" (
	"id" integer PRIMARY KEY NOT NULL,
	"sku" text NOT NULL,
	"stock" integer NOT NULL,
	CONSTRAINT "products_sku_unique" UNIQUE("sku")
);
CREATE TABLE "reservations" (
	"id" serial PRIMARY KEY NOT NULL,
	"product_id" integer NOT NULL,
	"quantity" integer NOT NULL
);
CREATE TABLE "orders" (
	"id" integer PRIMARY KEY NOT NULL,
	"customer_email" text NOT NULL,
	"amount" integer NOT NULL,
	"status" text DEFAULT 'unpaid' NOT NULL
);
CREATE TABLE "charges" (
	"id" serial PRIMARY KEY NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL
);
CREATE TABLE "payments" (
	"id" serial PRIMARY KEY NOT NULL,
	"order_id" integer NOT NULL,
	"reference" text NOT NULL,
	"amount" integer NOT NULL
);
CREATE TABLE "settled_payments" (
	"id" serial PRIMARY KEY NOT NULL,
	"order_id" integer NOT NULL,
	"reference" text NOT NULL,
	"amount" integer NOT NULL,
	CONSTRAINT "settled_payments_reference_unique" UNIQUE("reference")
);
CREATE TABLE "refund_jobs" (
	"id" integer PRIMARY KEY NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL,
	"status" text DEFAULT 'queued' NOT NULL
);
CREATE TABLE "job_effects" (
	"id" serial PRIMARY KEY NOT NULL,
	"job_id" integer NOT NULL,
	"effect" text NOT NULL
);
CREATE TABLE "tasks" (
	"id" integer PRIMARY KEY NOT NULL,
	"title" text NOT NULL,
	"status" text DEFAULT 'open' NOT NULL
);
CREATE TABLE "task_assignments" (
	"id" serial PRIMARY KEY NOT NULL,
	"task_id" integer NOT NULL,
	"assignee" text NOT NULL
);
CREATE TABLE "shifts" (
	"id" integer PRIMARY KEY NOT NULL,
	"store_id" integer NOT NULL,
	"manager_id" integer NOT NULL,
	"on_duty" boolean DEFAULT true NOT NULL
);
CREATE TABLE "bins" (
	"id" integer PRIMARY KEY NOT NULL,
	"quantity" integer NOT NULL
);
ALTER TABLE "members" ADD CONSTRAINT "members_org_id_organizations_id_fk" FOREIGN KEY ("org_id") REFERENCES "public"."organizations"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "invitations" ADD CONSTRAINT "invitations_org_id_organizations_id_fk" FOREIGN KEY ("org_id") REFERENCES "public"."organizations"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "invitations" ADD CONSTRAINT "invitations_invited_by_members_id_fk" FOREIGN KEY ("invited_by") REFERENCES "public"."members"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "seat_invites" ADD CONSTRAINT "seat_invites_org_id_organizations_id_fk" FOREIGN KEY ("org_id") REFERENCES "public"."organizations"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "reservations" ADD CONSTRAINT "reservations_product_id_products_id_fk" FOREIGN KEY ("product_id") REFERENCES "public"."products"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "charges" ADD CONSTRAINT "charges_order_id_orders_id_fk" FOREIGN KEY ("order_id") REFERENCES "public"."orders"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "payments" ADD CONSTRAINT "payments_order_id_orders_id_fk" FOREIGN KEY ("order_id") REFERENCES "public"."orders"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "settled_payments" ADD CONSTRAINT "settled_payments_order_id_orders_id_fk" FOREIGN KEY ("order_id") REFERENCES "public"."orders"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "refund_jobs" ADD CONSTRAINT "refund_jobs_order_id_orders_id_fk" FOREIGN KEY ("order_id") REFERENCES "public"."orders"("id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "task_assignments" ADD CONSTRAINT "task_assignments_task_id_tasks_id_fk" FOREIGN KEY ("task_id") REFERENCES "public"."tasks"("id") ON DELETE no action ON UPDATE no action;
CREATE UNIQUE INDEX "seat_invites_org_email" ON "seat_invites" USING btree ("org_id","email");
CREATE UNIQUE INDEX "job_effects_job_effect" ON "job_effects" USING btree ("job_id","effect");
