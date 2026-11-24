CREATE TABLE deleted_items (
  id          BLOB PRIMARY KEY CHECK (length(id) = 16),
  entity      TEXT NOT NULL CHECK (length(entity) BETWEEN 1 AND 40),
  entity_id   BLOB NOT NULL CHECK (length(entity_id) = 16),
  project_id  BLOB REFERENCES projects (id) ON DELETE CASCADE,
  human_key   TEXT,
  title       TEXT NOT NULL,
  from_status TEXT,
  actor_id    BLOB NOT NULL REFERENCES actors (id),
  at          INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_deleted_items_project ON deleted_items (project_id, at);
