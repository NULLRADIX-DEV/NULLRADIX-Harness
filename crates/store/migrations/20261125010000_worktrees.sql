CREATE TABLE worktrees (
  id             BLOB PRIMARY KEY CHECK (length(id) = 16),
  repository_id  BLOB NOT NULL REFERENCES repositories (id),
  runner_id      BLOB NOT NULL REFERENCES runners (id),
  branch         TEXT NOT NULL CHECK (length(branch) BETWEEN 1 AND 400),
  target_branch  TEXT CHECK (target_branch IS NULL OR length(target_branch) BETWEEN 1 AND 400),
  path           TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 500),
  purpose        TEXT NOT NULL DEFAULT 'work' CHECK (purpose IN ('work', 'research', 'plan')),
  desired_state  TEXT NOT NULL CHECK (desired_state IN ('present', 'absent')),
  actual_state   TEXT NOT NULL CHECK (actual_state IN ('unknown', 'creating', 'present',
                                                       'removing', 'removed', 'failed')),
  setup_state    TEXT NOT NULL DEFAULT 'none' CHECK (setup_state IN ('none', 'pending', 'done',
                                                                     'failed')),
  ready_to_merge INTEGER NOT NULL DEFAULT 0 CHECK (ready_to_merge IN (0, 1)),
  last_error     TEXT,
  observed_at    INTEGER,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL,
  version        INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

-- a removed entry keeps its row and branch name; only live ones must be unique
CREATE UNIQUE INDEX ux_worktrees_branch_live ON worktrees (repository_id, runner_id, branch)
  WHERE actual_state <> 'removed' OR desired_state = 'present';
-- registry order: --join and the dispatcher take the first match
CREATE INDEX ix_worktrees_registry ON worktrees (repository_id, runner_id, created_at, id);
CREATE INDEX ix_worktrees_reconcile ON worktrees (runner_id) WHERE desired_state <> actual_state;

-- legacy tickets[] of an entry: rows stay when the link ends or the ticket is deleted
CREATE TABLE worktree_tickets (
  worktree_id BLOB NOT NULL REFERENCES worktrees (id) ON DELETE CASCADE,
  ticket_id   BLOB NOT NULL CHECK (length(ticket_id) = 16),
  position    INTEGER NOT NULL CHECK (position >= 0),
  joined_at   INTEGER NOT NULL,
  released_at INTEGER,
  PRIMARY KEY (worktree_id, ticket_id),
  UNIQUE (worktree_id, position)
) STRICT;

CREATE UNIQUE INDEX ux_worktree_tickets_live ON worktree_tickets (ticket_id)
  WHERE released_at IS NULL;
CREATE INDEX ix_worktree_tickets_ticket ON worktree_tickets (ticket_id);
