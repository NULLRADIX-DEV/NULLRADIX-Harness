CREATE TABLE runners (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  name       TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
  os         TEXT NOT NULL CHECK (os IN ('windows', 'linux')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE TABLE repositories (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  name       TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE TABLE checkouts (
  id            BLOB PRIMARY KEY CHECK (length(id) = 16),
  repository_id BLOB NOT NULL REFERENCES repositories(id),
  runner_id     BLOB NOT NULL REFERENCES runners(id),
  path          TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 1000),
  path_key      TEXT NOT NULL CHECK (length(path_key) BETWEEN 1 AND 1000),
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  version       INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  UNIQUE (repository_id, runner_id),
  UNIQUE (runner_id, path_key)
) STRICT;

ALTER TABLE boards ADD COLUMN repository_id BLOB REFERENCES repositories(id);

CREATE INDEX ix_boards_repository ON boards (repository_id);
