-- AUTOINCREMENT: the rid is the note number people type, so it is never handed out twice
CREATE TABLE notes (
  rid                 INTEGER PRIMARY KEY AUTOINCREMENT,
  id                  BLOB NOT NULL UNIQUE CHECK (length(id) = 16),
  board_id            BLOB NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
  author_id           BLOB NOT NULL REFERENCES actors (id),
  title               TEXT NOT NULL CHECK (length(title) <= 200),
  body                TEXT NOT NULL,
  source_ticket_id    BLOB REFERENCES tickets (id) ON DELETE SET NULL,
  source_ticket_key   TEXT CHECK (source_ticket_key IS NULL OR length(source_ticket_key) BETWEEN 1 AND 20),
  state               TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'promoted', 'dropped')),
  promoted_ticket_id  BLOB REFERENCES tickets (id) ON DELETE SET NULL,
  promoted_ticket_key TEXT CHECK (promoted_ticket_key IS NULL
                                  OR length(promoted_ticket_key) BETWEEN 1 AND 20),
  reason              TEXT CHECK (reason IS NULL OR length(reason) BETWEEN 1 AND 1000),
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  CHECK ((state = 'promoted') = (promoted_ticket_key IS NOT NULL)),
  CHECK ((state = 'dropped') = (reason IS NOT NULL)),
  CHECK (source_ticket_id IS NULL OR source_ticket_key IS NOT NULL),
  CHECK (promoted_ticket_id IS NULL OR promoted_ticket_key IS NOT NULL)
) STRICT;

CREATE INDEX ix_notes_board_state ON notes (board_id, state);

CREATE TABLE note_labels (
  note_id  BLOB NOT NULL REFERENCES notes (id) ON DELETE CASCADE,
  label    TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 100),
  position INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (note_id, label)
) STRICT;

-- legacy SeenCount/SeenIn: seen count is 1 + rows; the key stays when the ticket is deleted
CREATE TABLE note_sightings (
  note_id    BLOB NOT NULL REFERENCES notes (id) ON DELETE CASCADE,
  position   INTEGER NOT NULL CHECK (position >= 0),
  ticket_id  BLOB REFERENCES tickets (id) ON DELETE SET NULL,
  ticket_key TEXT CHECK (ticket_key IS NULL OR length(ticket_key) BETWEEN 1 AND 20),
  seen_at    INTEGER NOT NULL,
  PRIMARY KEY (note_id, position),
  CHECK (ticket_id IS NULL OR ticket_key IS NOT NULL)
) STRICT;
