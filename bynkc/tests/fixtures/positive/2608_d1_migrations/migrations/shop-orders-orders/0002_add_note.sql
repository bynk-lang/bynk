-- #2608: a later migration changes it, and seeds a row.
ALTER TABLE orders ADD COLUMN note TEXT NOT NULL DEFAULT '';
INSERT INTO orders (id, total, note) VALUES ('seed', 1, 'it''s seeded');
