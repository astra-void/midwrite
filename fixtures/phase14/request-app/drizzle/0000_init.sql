-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "orders" (
	"id" integer PRIMARY KEY NOT NULL,
	"status" text NOT NULL
);
CREATE TABLE "order_items" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"order_id" integer NOT NULL,
	"request_id" text NOT NULL,
	"sku" text NOT NULL,
	"qty" integer NOT NULL
);
CREATE TABLE "request_items" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"order_id" integer NOT NULL,
	"request_id" text NOT NULL,
	"sku" text NOT NULL,
	"qty" integer NOT NULL
);
CREATE UNIQUE INDEX "request_items_request_sku" ON "request_items" USING btree ("request_id","sku");
CREATE TABLE "payments" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL
);
