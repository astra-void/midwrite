-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "orders" (
	"id" serial PRIMARY KEY NOT NULL,
	"request_id" text NOT NULL,
	"customer_id" integer NOT NULL,
	"amount" integer NOT NULL
);
CREATE TABLE "payments" (
	"id" serial PRIMARY KEY NOT NULL,
	"idempotency_key" text NOT NULL,
	"order_id" integer NOT NULL,
	"amount" integer NOT NULL,
	CONSTRAINT "payments_idempotency_key_unique" UNIQUE("idempotency_key")
);
CREATE TABLE "accounts" (
	"id" integer PRIMARY KEY NOT NULL,
	"balance" integer NOT NULL
);
