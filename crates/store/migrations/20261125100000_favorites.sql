-- a snapshot that outlives the pruned session, hence no reference to claude_sessions
CREATE TABLE favorites (
  session_id    TEXT PRIMARY KEY CHECK (length(session_id) BETWEEN 1 AND 200),
  -- entity id in the change feed, which only takes UUIDs
  feed_id       BLOB NOT NULL UNIQUE CHECK (length(feed_id) = 16),
  repository_id BLOB REFERENCES repositories (id) ON DELETE CASCADE,
  note          TEXT NOT NULL DEFAULT '' CHECK (length(note) <= 2000),
  cwd           TEXT NOT NULL CHECK (length(cwd) BETWEEN 1 AND 4096),
  title         TEXT,
  ticket_keys   TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(ticket_keys)),
  last_state    TEXT NOT NULL CHECK (last_state IN ('idle', 'running', 'waiting', 'ended')),
  last_event_at INTEGER NOT NULL,
  marked_at     INTEGER NOT NULL,
  version       INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE INDEX ix_favorites_marked ON favorites (marked_at DESC);
CREATE INDEX ix_favorites_repository ON favorites (repository_id, marked_at DESC);

CREATE TABLE favorite_resumes (
  favorite_session_id TEXT NOT NULL REFERENCES favorites (session_id) ON DELETE CASCADE,
  resumed_session_id  TEXT NOT NULL CHECK (length(resumed_session_id) BETWEEN 1 AND 200),
  -- order of the resumptions as they came
  position            INTEGER NOT NULL CHECK (position >= 0),
  resumed_at          INTEGER NOT NULL,
  PRIMARY KEY (favorite_session_id, resumed_session_id),
  UNIQUE (favorite_session_id, position)
) STRICT;
