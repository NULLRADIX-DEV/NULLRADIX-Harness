CREATE TABLE runs (
  id          BLOB PRIMARY KEY CHECK (length(id) = 16),
  label       TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 200),
  status      TEXT NOT NULL
              CHECK (status IN ('starting', 'running', 'succeeded', 'failed', 'cancelled', 'timed-out', 'lost')),
  pid         INTEGER CHECK (pid > 0),
  exit_code   INTEGER,
  error       TEXT,
  created_at  INTEGER NOT NULL,
  started_at  INTEGER,
  finished_at INTEGER,
  version     INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE INDEX ix_runs_created_at ON runs (created_at DESC);
CREATE INDEX ix_runs_active ON runs (status) WHERE status IN ('starting', 'running');
