CREATE TABLE invoices (
  id text PRIMARY KEY,
  status text NOT NULL
);

INSERT INTO invoices (id, status) VALUES ('inv-1', 'draft');
