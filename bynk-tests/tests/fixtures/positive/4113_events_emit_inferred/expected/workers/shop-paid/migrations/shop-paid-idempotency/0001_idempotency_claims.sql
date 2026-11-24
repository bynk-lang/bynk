CREATE TABLE idempotency_claims (
  key TEXT PRIMARY KEY NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE INDEX idempotency_claims_expires_at ON idempotency_claims (expires_at);
