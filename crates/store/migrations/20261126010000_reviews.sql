-- The review before merge: a finished worktree waits here for a person, with the tests the agent ran
CREATE TABLE reviews (
  id          BLOB PRIMARY KEY CHECK (length(id) = 16),
  worktree_id BLOB NOT NULL REFERENCES worktrees (id) ON DELETE CASCADE,
  run_id      BLOB REFERENCES runs (id) ON DELETE SET NULL,
  branch      TEXT NOT NULL CHECK (length(branch) BETWEEN 1 AND 400),
  state       TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'returned')),
  decided_by  BLOB REFERENCES actors (id),
  decided_at  INTEGER,
  created_at  INTEGER NOT NULL,
  version     INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

-- one open review per worktree, and one review per run, so a settle that repeats adds nothing
CREATE UNIQUE INDEX ux_reviews_pending ON reviews (worktree_id) WHERE state = 'pending';
CREATE UNIQUE INDEX ux_reviews_run ON reviews (run_id) WHERE run_id IS NOT NULL;
CREATE INDEX ix_reviews_worktree ON reviews (worktree_id, created_at);

CREATE TABLE review_test_runs (
  review_id BLOB NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
  position  INTEGER NOT NULL CHECK (position >= 0),
  command   TEXT NOT NULL,
  ok        INTEGER CHECK (ok IS NULL OR ok IN (0, 1)),
  output    TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (review_id, position)
) STRICT;
