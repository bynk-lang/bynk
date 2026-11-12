-- The project keeps a Postgres schema, which switches the statement check on.
CREATE TABLE orders (id serial PRIMARY KEY, name text NOT NULL, qty bigint NOT NULL DEFAULT 1);
