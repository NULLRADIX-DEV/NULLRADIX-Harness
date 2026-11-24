CREATE TABLE ticket_steps (
  id        BLOB PRIMARY KEY CHECK (length(id) = 16),
  ticket_id BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  position  INTEGER NOT NULL CHECK (position >= 0),
  text      TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 1000),
  done      INTEGER NOT NULL CHECK (done IN (0, 1)),
  detail    TEXT CHECK (detail IS NULL OR length(detail) >= 1),
  UNIQUE (ticket_id, position)
) STRICT;
