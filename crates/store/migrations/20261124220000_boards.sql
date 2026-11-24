CREATE TABLE boards (
  id               BLOB PRIMARY KEY CHECK (length(id) = 16),
  project_id       BLOB NOT NULL REFERENCES projects(id),
  name             TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  owner_id         BLOB NOT NULL REFERENCES actors(id),
  dispatch_enabled INTEGER NOT NULL DEFAULT 1 CHECK (dispatch_enabled IN (0, 1)),
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  version          INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  UNIQUE (id, project_id)
) STRICT;

CREATE INDEX ix_boards_project ON boards (project_id);
