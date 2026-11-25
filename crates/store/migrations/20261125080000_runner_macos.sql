-- no-transaction
-- rebuilds runners (sqlite.org/lang_altertable.html) with foreign keys off so no referencing row changes
PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

CREATE TABLE runners_new (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  name       TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
  os         TEXT NOT NULL CHECK (os IN ('windows', 'linux', 'macos')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

INSERT INTO runners_new (id, name, os, created_at, updated_at, version)
SELECT id, name, os, created_at, updated_at, version FROM runners;

DROP TABLE runners;

ALTER TABLE runners_new RENAME TO runners;

-- overflows and aborts when a reference dangles
SELECT abs(-9223372036854775807 - 1) FROM pragma_foreign_key_check LIMIT 1;

COMMIT;

PRAGMA foreign_keys = ON;
