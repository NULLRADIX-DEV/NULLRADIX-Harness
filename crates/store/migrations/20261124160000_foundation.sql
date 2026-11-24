CREATE TABLE actors (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  kind       TEXT NOT NULL CHECK (kind IN ('human', 'session', 'subagent', 'dispatcher', 'system')),
  name       TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
  created_at INTEGER NOT NULL,
  UNIQUE (kind, name)
) STRICT;

CREATE TABLE settings (
  key        TEXT PRIMARY KEY CHECK (length(key) BETWEEN 1 AND 100),
  value      TEXT NOT NULL CHECK (json_valid(value)),
  version    INTEGER NOT NULL DEFAULT 1,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE change_feed (
  seq       INTEGER PRIMARY KEY AUTOINCREMENT,
  entity    TEXT NOT NULL CHECK (length(entity) BETWEEN 1 AND 40),
  entity_id BLOB NOT NULL CHECK (length(entity_id) = 16),
  op        TEXT NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  at        INTEGER NOT NULL
) STRICT;
