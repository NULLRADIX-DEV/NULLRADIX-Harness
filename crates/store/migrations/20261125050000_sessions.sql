ALTER TABLE repositories ADD COLUMN next_session_number INTEGER NOT NULL DEFAULT 1
  CHECK (next_session_number >= 1);

-- one row per Claude Code session, keyed by its id; `sessions` holds browser logins
CREATE TABLE claude_sessions (
  id              TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 200),
  -- entity id in the change feed, which only takes UUIDs
  feed_id         BLOB NOT NULL UNIQUE CHECK (length(feed_id) = 16),
  repository_id   BLOB REFERENCES repositories (id) ON DELETE SET NULL,
  runner_id       BLOB REFERENCES runners (id) ON DELETE SET NULL,
  number          INTEGER CHECK (number IS NULL OR number >= 1),
  cwd             TEXT NOT NULL CHECK (length(cwd) BETWEEN 1 AND 4096),
  state           TEXT NOT NULL CHECK (state IN ('idle', 'running', 'waiting', 'ended')),
  client          TEXT NOT NULL DEFAULT 'unknown'
                  CHECK (client IN ('nullradix', 'claude-code', 'claude-desktop', 'unknown')),
  title           TEXT,
  model           TEXT,
  transcript_path TEXT,
  run_name        TEXT,
  started_at      INTEGER NOT NULL,
  last_event_at   INTEGER NOT NULL,
  ended_at        INTEGER,
  -- write order, so sessions with the same last event list the latest report first
  touched         INTEGER NOT NULL,
  version         INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  UNIQUE (repository_id, number)
) STRICT;

CREATE INDEX ix_claude_sessions_activity ON claude_sessions (last_event_at DESC, touched DESC);
CREATE INDEX ix_claude_sessions_repository ON claude_sessions (repository_id, last_event_at DESC);
CREATE INDEX ix_claude_sessions_unlocated ON claude_sessions (cwd) WHERE repository_id IS NULL;

CREATE TABLE session_subagents (
  session_id TEXT NOT NULL REFERENCES claude_sessions (id) ON DELETE CASCADE,
  agent_id   TEXT NOT NULL CHECK (length(agent_id) BETWEEN 1 AND 1000),
  position   INTEGER NOT NULL CHECK (position >= 0),
  agent_type TEXT NOT NULL CHECK (length(agent_type) BETWEEN 1 AND 1000),
  state      TEXT NOT NULL CHECK (state IN ('running', 'ended')),
  started_at INTEGER NOT NULL,
  ended_at   INTEGER,
  PRIMARY KEY (session_id, agent_id),
  UNIQUE (session_id, position)
) STRICT;

CREATE TABLE ticket_sessions (
  ticket_id  BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  session_id TEXT NOT NULL REFERENCES claude_sessions (id) ON DELETE CASCADE,
  added_at   INTEGER NOT NULL,
  PRIMARY KEY (ticket_id, session_id)
) STRICT;

CREATE INDEX ix_ticket_sessions_session ON ticket_sessions (session_id);
