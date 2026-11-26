-- Runtime state of the dispatcher: the switch, the pause and the last pass, its log and auto runs
CREATE TABLE dispatcher_state (
  id               INTEGER PRIMARY KEY CHECK (id = 1),
  enabled_override INTEGER CHECK (enabled_override IS NULL OR enabled_override IN (0, 1)),
  paused_until     INTEGER,
  last_pass_at     INTEGER,
  last_pass        TEXT CHECK (last_pass IS NULL OR json_valid(last_pass)),
  version          INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

INSERT INTO dispatcher_state (id) VALUES (1);

-- how often research and planning ran for a question or ticket
CREATE TABLE auto_run_tracking (
  kind            TEXT NOT NULL CHECK (kind IN ('research', 'planning', 'autopilot-research')),
  subject_id      BLOB NOT NULL CHECK (length(subject_id) = 16),
  count           INTEGER NOT NULL DEFAULT 0 CHECK (count >= 0),
  last_started_at INTEGER,
  auto_at         INTEGER,
  PRIMARY KEY (kind, subject_id)
) STRICT;

CREATE TABLE dispatcher_log (
  id          BLOB PRIMARY KEY CHECK (length(id) = 16),
  at          INTEGER NOT NULL,
  level       TEXT NOT NULL CHECK (level IN ('debug', 'info', 'warn', 'error')),
  message     TEXT NOT NULL,
  run_id      BLOB REFERENCES runs (id) ON DELETE SET NULL,
  subject_key TEXT
) STRICT;

CREATE INDEX ix_dispatcher_log_at ON dispatcher_log (at);

-- a paused account is skipped by the account choice until the pause ends
CREATE TABLE account_pauses (
  account_id BLOB PRIMARY KEY REFERENCES claude_accounts (id) ON DELETE CASCADE,
  until      INTEGER NOT NULL,
  reason     TEXT NOT NULL CHECK (reason IN ('usage-limit', 'auth-missing', 'manual')),
  run_id     BLOB REFERENCES runs (id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL
) STRICT;
