CREATE TABLE hook_events (
  id                BLOB PRIMARY KEY CHECK (length(id) = 16),
  kind              TEXT NOT NULL
                    CHECK (kind IN ('session-start', 'user-prompt', 'notification', 'stop',
                                    'subagent-start', 'subagent-stop', 'session-end')),
  session_id        TEXT NOT NULL CHECK (length(session_id) BETWEEN 1 AND 200),
  cwd               TEXT NOT NULL CHECK (length(cwd) BETWEEN 1 AND 4096),
  occurred_at       INTEGER NOT NULL,
  received_at       INTEGER NOT NULL,
  transcript_path   TEXT,
  prompt            TEXT,
  source            TEXT,
  model             TEXT,
  reason            TEXT,
  message           TEXT,
  notification_type TEXT,
  agent_id          TEXT,
  agent_type        TEXT,
  ticket            TEXT,
  run               TEXT,
  entrypoint        TEXT
) STRICT;

CREATE INDEX ix_hook_events_session ON hook_events (session_id, occurred_at);
CREATE INDEX ix_hook_events_occurred ON hook_events (occurred_at DESC);
CREATE INDEX ix_hook_events_received ON hook_events (received_at);
