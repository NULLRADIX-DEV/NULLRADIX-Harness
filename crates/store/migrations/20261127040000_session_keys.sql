-- Sessions now belong to one browser tab through a key; old ones cannot be used, so people sign in again.
DROP TABLE sessions;

CREATE TABLE sessions (
  id             BLOB PRIMARY KEY CHECK (length(id) = 16),
  hash           TEXT NOT NULL UNIQUE CHECK (length(hash) = 64),
  key_hash       TEXT NOT NULL CHECK (length(key_hash) = 64),
  created_at     INTEGER NOT NULL,
  expires_at     INTEGER NOT NULL,
  last_seen_at   INTEGER NOT NULL,
  elevated_until INTEGER
) STRICT;

CREATE INDEX ix_sessions_expires_at ON sessions (expires_at);
