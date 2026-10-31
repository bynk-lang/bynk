-- #2608: the first migration creates the table the services use.
CREATE TABLE orders (
  id TEXT PRIMARY KEY,
  total INTEGER NOT NULL CHECK (total >= 0)
);
