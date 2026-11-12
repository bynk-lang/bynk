-- The project's Postgres schema, kept by hand: Bynk does not own it. The
-- PGlite double applies it to each case's fresh database.
CREATE TABLE orders (
	id serial PRIMARY KEY,
	name text NOT NULL UNIQUE,
	qty bigint NOT NULL DEFAULT 1,
	placed timestamptz NOT NULL DEFAULT '2026-11-12T10:20:30Z'
);
