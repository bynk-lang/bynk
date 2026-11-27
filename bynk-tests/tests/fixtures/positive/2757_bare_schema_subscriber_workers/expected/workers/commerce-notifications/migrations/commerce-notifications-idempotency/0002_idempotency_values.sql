CREATE TABLE idempotency_values (
  key TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE INDEX idempotency_values_expires_at ON idempotency_values (expires_at);
