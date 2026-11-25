-- ticket comments only; questions will rebuild the table with a nullable ticket_id
CREATE TABLE comments (
  rid                   INTEGER PRIMARY KEY,
  id                    BLOB NOT NULL UNIQUE CHECK (length(id) = 16),
  ticket_id             BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  author_id             BLOB NOT NULL REFERENCES actors (id),
  body                  TEXT NOT NULL CHECK (length(body) >= 1),
  system_marker         TEXT CHECK (system_marker IN ('start', 'abort', 'pause', 'question',
                                                      'merged', 'merge_conflict', 'merge_wait',
                                                      'merge', 'setup', 'research_empty',
                                                      'plan_empty', 'review', 'review_returned')),
  run_id                BLOB REFERENCES runs (id) ON DELETE SET NULL,
  delivered_to_agent_at INTEGER,
  created_at            INTEGER NOT NULL,
  updated_at            INTEGER NOT NULL,
  version               INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE INDEX ix_comments_ticket ON comments (ticket_id, created_at);
CREATE INDEX ix_comments_undelivered ON comments (ticket_id) WHERE delivered_to_agent_at IS NULL;
