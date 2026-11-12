CREATE TABLE orders (
	id serial PRIMARY KEY,
	qty bigint NOT NULL DEFAULT 1,
	placed timestamptz NOT NULL DEFAULT '2026-11-12T10:20:30Z'
);
