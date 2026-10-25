-- Initial state of every run.
CREATE TABLE orders (
  id text PRIMARY KEY,
  status text NOT NULL
);

INSERT INTO orders (id, status) VALUES ('order-1', 'pending');
