-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/lib/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "payments" (
	"id" serial PRIMARY KEY NOT NULL,
	"request_id" text NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL
);
CREATE TABLE "ledger" (
	"id" serial PRIMARY KEY NOT NULL,
	"request_id" text NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL,
	CONSTRAINT "ledger_request_id_unique" UNIQUE("request_id")
);
CREATE TABLE "orders" (
	"id" integer PRIMARY KEY NOT NULL,
	"status" text DEFAULT 'unpaid' NOT NULL
);
