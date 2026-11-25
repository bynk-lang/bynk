-- #4200: `amount` is TEXT, which keeps a `Decimal` exact. `tax` is NUMERIC, whose
-- affinity hands a value back as a number, so reading it as a `Decimal` warns.
CREATE TABLE prices (
  id TEXT PRIMARY KEY,
  amount TEXT NOT NULL,
  tax NUMERIC
);
