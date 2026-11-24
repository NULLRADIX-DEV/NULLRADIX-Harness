CREATE TABLE projects (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  key        TEXT NOT NULL UNIQUE
             CHECK (length(key) BETWEEN 2 AND 6 AND key GLOB '[A-Z]*' AND key NOT GLOB '*[^A-Z0-9]*'),
  name       TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;
