CREATE TABLE api_tokens (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  name       TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
  kind       TEXT NOT NULL CHECK (kind IN ('cli')),
  hash       TEXT NOT NULL UNIQUE CHECK (length(hash) = 64),
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE sessions (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  hash       TEXT NOT NULL UNIQUE CHECK (length(hash) = 64),
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_sessions_expires_at ON sessions (expires_at);
