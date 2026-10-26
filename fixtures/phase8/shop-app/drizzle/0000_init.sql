-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/lib/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "orders" (
	"id" integer PRIMARY KEY NOT NULL,
	"customer_email" text NOT NULL,
	"amount" integer NOT NULL,
	"status" text DEFAULT 'unpaid' NOT NULL,
	"updated_at" timestamp DEFAULT now() NOT NULL
);
CREATE TABLE "payment_attempts" (
	"id" serial PRIMARY KEY NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL
);
CREATE TABLE "idempotency_keys" (
	"key" text PRIMARY KEY NOT NULL,
	"order_id" integer NOT NULL,
	"status" text DEFAULT 'started' NOT NULL
);
CREATE TABLE "outbox" (
	"id" serial PRIMARY KEY NOT NULL,
	"topic" text NOT NULL,
	"order_id" integer NOT NULL,
	"published" boolean DEFAULT false NOT NULL
);
CREATE TABLE "jobs" (
	"id" integer PRIMARY KEY NOT NULL,
	"kind" text NOT NULL,
	"order_id" integer NOT NULL,
	"status" text DEFAULT 'queued' NOT NULL
);
CREATE TABLE "job_effects" (
	"id" serial PRIMARY KEY NOT NULL,
	"job_id" integer NOT NULL,
	"effect" text NOT NULL
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
ALTER TABLE "payment_attempts" ADD CONSTRAINT "payment_attempts_order_id_orders_id_fk" FOREIGN KEY ("order_id") REFERENCES "public"."orders"("id");
ALTER TABLE "idempotency_keys" ADD CONSTRAINT "idempotency_keys_order_id_orders_id_fk" FOREIGN KEY ("order_id") REFERENCES "public"."orders"("id");
ALTER TABLE "reservations" ADD CONSTRAINT "reservations_product_id_products_id_fk" FOREIGN KEY ("product_id") REFERENCES "public"."products"("id");
CREATE UNIQUE INDEX "job_effects_job_effect_unique" ON "job_effects" USING btree ("job_id","effect");
