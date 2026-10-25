-- Initial state of every run.
CREATE TABLE orders (
  id text PRIMARY KEY,
  status text NOT NULL
);

CREATE TABLE outbox (
  id bigserial PRIMARY KEY,
  topic text NOT NULL,
  payload text NOT NULL,
  published_at timestamptz
);

INSERT INTO orders (id, status) VALUES ('order-1', 'pending');
