ALTER TABLE projects
  ADD COLUMN next_ticket_number INTEGER NOT NULL DEFAULT 1 CHECK (next_ticket_number >= 1);

CREATE TABLE tickets (
  rid            INTEGER PRIMARY KEY,
  id             BLOB NOT NULL UNIQUE CHECK (length(id) = 16),
  project_id     BLOB NOT NULL REFERENCES projects (id),
  board_id       BLOB NOT NULL,
  number         INTEGER NOT NULL CHECK (number >= 1),
  title          TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 400),
  description    TEXT NOT NULL,
  acceptance     TEXT NOT NULL,
  plan           TEXT NOT NULL,
  handbook       TEXT NOT NULL,
  diagram        TEXT NOT NULL,
  status         TEXT NOT NULL CHECK (status IN ('backlog', 'in-planning', 'planned', 'in-progress',
                                                 'blocked', 'done', 'not-needed')),
  priority       TEXT NOT NULL CHECK (priority IN ('low', 'normal', 'high', 'urgent')),
  sort_order     INTEGER NOT NULL CHECK (sort_order >= 0),
  assignee       TEXT CHECK (assignee IS NULL OR length(assignee) BETWEEN 1 AND 100),
  needs_planning INTEGER NOT NULL CHECK (needs_planning IN (0, 1)),
  blocked_reason TEXT CHECK (blocked_reason IS NULL OR length(blocked_reason) BETWEEN 1 AND 500),
  target_branch  TEXT CHECK (target_branch IS NULL OR length(target_branch) BETWEEN 1 AND 400),
  created_by     BLOB NOT NULL REFERENCES actors (id),
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL,
  version        INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  UNIQUE (project_id, number),
  FOREIGN KEY (board_id, project_id) REFERENCES boards (id, project_id)
) STRICT;

CREATE INDEX ix_tickets_board_status ON tickets (board_id, status, sort_order);

CREATE TABLE ticket_labels (
  ticket_id BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  label     TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 100),
  position  INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (ticket_id, label)
) STRICT;

CREATE INDEX ix_ticket_labels_label ON ticket_labels (label);

CREATE TABLE ticket_dependencies (
  ticket_id     BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  depends_on_id BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  position      INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (ticket_id, depends_on_id),
  CHECK (ticket_id <> depends_on_id)
) STRICT;

CREATE INDEX ix_ticket_dependencies_depends_on ON ticket_dependencies (depends_on_id);

CREATE TABLE activity (
  id         BLOB PRIMARY KEY CHECK (length(id) = 16),
  ticket_id  BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  actor_id   BLOB NOT NULL REFERENCES actors (id),
  at         INTEGER NOT NULL,
  field      TEXT NOT NULL CHECK (length(field) BETWEEN 1 AND 40),
  from_value TEXT,
  to_value   TEXT,
  note       TEXT
) STRICT;

CREATE INDEX ix_activity_ticket ON activity (ticket_id, at);
