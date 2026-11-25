ALTER TABLE projects
  ADD COLUMN next_question_number INTEGER NOT NULL DEFAULT 1 CHECK (next_question_number >= 1);

CREATE TABLE questions (
  rid             INTEGER PRIMARY KEY,
  id              BLOB NOT NULL UNIQUE CHECK (length(id) = 16),
  project_id      BLOB NOT NULL REFERENCES projects (id),
  number          INTEGER NOT NULL CHECK (number >= 1),
  title           TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 400),
  description     TEXT NOT NULL DEFAULT '',
  state           TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'answered', 'discarded')),
  occasion        TEXT CHECK (occasion IS NULL OR length(occasion) BETWEEN 1 AND 400),
  origin_actor_id BLOB NOT NULL REFERENCES actors (id),
  -- the agent session that asked; no FK until sessions are stored
  origin_session  TEXT CHECK (origin_session IS NULL OR length(origin_session) BETWEEN 1 AND 100),
  decision_option TEXT CHECK (decision_option IS NULL OR decision_option GLOB 'O[1-9]*'),
  decision_text   TEXT,
  decision_reason TEXT,
  decided_by      BLOB REFERENCES actors (id),
  decided_at      INTEGER,
  created_by      BLOB NOT NULL REFERENCES actors (id),
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  version         INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  UNIQUE (project_id, number),
  CHECK ((state = 'open') = (decided_at IS NULL)),
  CHECK ((state = 'open') = (decided_by IS NULL)),
  CHECK (state <> 'discarded' OR decision_reason IS NOT NULL),
  CHECK (state <> 'answered' OR (decision_option IS NULL) <> (decision_text IS NULL))
) STRICT;

CREATE INDEX ix_questions_project_state ON questions (project_id, state);

CREATE TABLE question_tags (
  question_id BLOB NOT NULL REFERENCES questions (id) ON DELETE CASCADE,
  tag         TEXT NOT NULL CHECK (length(tag) BETWEEN 1 AND 100),
  position    INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (question_id, tag)
) STRICT;

-- the tickets a question is about; an open question holds them back
CREATE TABLE question_tickets (
  question_id BLOB NOT NULL REFERENCES questions (id) ON DELETE CASCADE,
  ticket_id   BLOB NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
  PRIMARY KEY (question_id, ticket_id)
) STRICT;

CREATE INDEX ix_question_tickets_ticket ON question_tickets (ticket_id);

CREATE TABLE question_dependencies (
  question_id            BLOB NOT NULL REFERENCES questions (id) ON DELETE CASCADE,
  depends_on_question_id BLOB REFERENCES questions (id) ON DELETE CASCADE,
  depends_on_ticket_id   BLOB REFERENCES tickets (id) ON DELETE CASCADE,
  position               INTEGER NOT NULL CHECK (position >= 0),
  CHECK ((depends_on_question_id IS NULL) <> (depends_on_ticket_id IS NULL)),
  CHECK (depends_on_question_id IS NULL OR depends_on_question_id <> question_id)
) STRICT;

CREATE UNIQUE INDEX ux_question_dependencies_question
  ON question_dependencies (question_id, depends_on_question_id)
  WHERE depends_on_question_id IS NOT NULL;
CREATE UNIQUE INDEX ux_question_dependencies_ticket
  ON question_dependencies (question_id, depends_on_ticket_id)
  WHERE depends_on_ticket_id IS NOT NULL;
CREATE INDEX ix_question_dependencies_ticket ON question_dependencies (depends_on_ticket_id);
CREATE INDEX ix_question_dependencies_on_question
  ON question_dependencies (depends_on_question_id);

-- a round of options; asking with options writes the first one
CREATE TABLE question_research (
  id          BLOB PRIMARY KEY CHECK (length(id) = 16),
  question_id BLOB NOT NULL REFERENCES questions (id) ON DELETE CASCADE,
  actor_id    BLOB NOT NULL REFERENCES actors (id),
  run_id      BLOB REFERENCES runs (id) ON DELETE SET NULL,
  context     TEXT NOT NULL DEFAULT '',
  created_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_question_research_question ON question_research (question_id, created_at);

-- O1, O2 ... count on across the rounds of one question
CREATE TABLE question_options (
  question_id BLOB NOT NULL REFERENCES questions (id) ON DELETE CASCADE,
  research_id BLOB NOT NULL REFERENCES question_research (id) ON DELETE CASCADE,
  number      INTEGER NOT NULL CHECK (number >= 1),
  title       TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 400),
  text        TEXT NOT NULL CHECK (length(text) >= 1),
  pros        TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(pros) AND json_type(pros) = 'array'),
  cons        TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(cons) AND json_type(cons) = 'array'),
  recommended INTEGER NOT NULL DEFAULT 0 CHECK (recommended IN (0, 1)),
  PRIMARY KEY (question_id, number)
) STRICT;

CREATE INDEX ix_question_options_research ON question_options (research_id, number);

-- comments belong to a ticket or a question; the rebuild keeps every ticket comment
CREATE TABLE comments_new (
  rid                   INTEGER PRIMARY KEY,
  id                    BLOB NOT NULL UNIQUE CHECK (length(id) = 16),
  ticket_id             BLOB REFERENCES tickets (id) ON DELETE CASCADE,
  question_id           BLOB REFERENCES questions (id) ON DELETE CASCADE,
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
  version               INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  CHECK ((ticket_id IS NULL) <> (question_id IS NULL))
) STRICT;

INSERT INTO comments_new (rid, id, ticket_id, question_id, author_id, body, system_marker, run_id,
                          delivered_to_agent_at, created_at, updated_at, version)
SELECT rid, id, ticket_id, NULL, author_id, body, system_marker, run_id, delivered_to_agent_at,
       created_at, updated_at, version
FROM comments;

DROP TABLE comments;
ALTER TABLE comments_new RENAME TO comments;

CREATE INDEX ix_comments_ticket ON comments (ticket_id, created_at) WHERE ticket_id IS NOT NULL;
CREATE INDEX ix_comments_question ON comments (question_id, created_at)
  WHERE question_id IS NOT NULL;
CREATE INDEX ix_comments_undelivered ON comments (ticket_id)
  WHERE delivered_to_agent_at IS NULL AND ticket_id IS NOT NULL;
