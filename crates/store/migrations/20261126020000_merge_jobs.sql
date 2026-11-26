-- One merge job per worktree: the branch of its tickets goes into the target branch
CREATE TABLE merge_jobs (
  id                BLOB PRIMARY KEY CHECK (length(id) = 16),
  worktree_id       BLOB NOT NULL REFERENCES worktrees (id) ON DELETE CASCADE,
  phase             TEXT NOT NULL CHECK (phase IN ('pending', 'waiting', 'conflict_session', 'merged',
                                                   'failed', 'cancelled')),
  -- none: the branch the main checkout has checked out
  target_branch     TEXT CHECK (target_branch IS NULL OR length(target_branch) BETWEEN 1 AND 400),
  target_head       TEXT,
  strategy          TEXT NOT NULL CHECK (strategy IN ('merge', 'squash')),
  conflict_sessions INTEGER NOT NULL DEFAULT 0 CHECK (conflict_sessions >= 0),
  -- a session worked on the conflict already; reset after a failed autopilot session
  session_used      INTEGER NOT NULL DEFAULT 0 CHECK (session_used IN (0, 1)),
  conflict_seen     INTEGER NOT NULL DEFAULT 0 CHECK (conflict_seen IN (0, 1)),
  wait_noted        INTEGER NOT NULL DEFAULT 0 CHECK (wait_noted IN (0, 1)),
  held_by           TEXT,
  hold_noticed      INTEGER NOT NULL DEFAULT 0 CHECK (hold_noticed IN (0, 1)),
  -- why it failed, or the files of the conflict it waits on
  problem           TEXT CHECK (problem IS NULL OR length(problem) <= 2000),
  merged_into       TEXT,
  merged_commit     TEXT,
  since             INTEGER NOT NULL,
  finished_at       INTEGER,
  version           INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE UNIQUE INDEX ux_merge_jobs_active ON merge_jobs (worktree_id)
  WHERE phase IN ('pending', 'waiting', 'conflict_session');
CREATE INDEX ix_merge_jobs_worktree ON merge_jobs (worktree_id, since);
