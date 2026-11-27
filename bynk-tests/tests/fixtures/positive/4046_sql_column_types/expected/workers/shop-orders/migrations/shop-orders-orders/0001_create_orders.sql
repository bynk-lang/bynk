-- #4046: `placed` and `wait` are INTEGER milliseconds, `total` is canonical TEXT and `status`
-- is the variant's name. `logged` is TEXT, so reading it as an `Instant` warns.
CREATE TABLE orders (
  id TEXT PRIMARY KEY,
  total TEXT NOT NULL,
  placed INTEGER NOT NULL,
  wait INTEGER NOT NULL,
  status TEXT NOT NULL,
  logged TEXT
);
