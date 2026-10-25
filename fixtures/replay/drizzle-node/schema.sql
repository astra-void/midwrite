-- Initial state of every run (applied to a fresh database).
CREATE TABLE orders (
  id text PRIMARY KEY,
  status text NOT NULL
);

CREATE TABLE payments (
  id serial PRIMARY KEY,
  order_id text NOT NULL REFERENCES orders (id),
  amount_cents integer NOT NULL
);

-- Tracking numbers are unique, checked at COMMIT (deferred): a duplicate
-- makes the COMMIT itself fail, after the transaction callback returned.
CREATE TABLE shipments (
  order_id text PRIMARY KEY REFERENCES orders (id),
  tracking_number text NOT NULL,
  CONSTRAINT shipments_tracking_number_key UNIQUE (tracking_number) DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE outbox (
  id text PRIMARY KEY,
  topic text NOT NULL,
  payload jsonb NOT NULL,
  published_at timestamptz
);

INSERT INTO orders (id, status) VALUES
  ('order-0', 'shipped'),
  ('order-1', 'pending'),
  ('order-2', 'pending'),
  ('order-3', 'pending'),
  ('order-4', 'pending');

INSERT INTO shipments (order_id, tracking_number) VALUES ('order-0', 'TRACK-TAKEN');
