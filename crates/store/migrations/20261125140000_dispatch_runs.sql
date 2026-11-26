-- Dispatch data of a run; plain runs keep these columns empty
ALTER TABLE runs ADD COLUMN kind TEXT CHECK (kind IS NULL OR kind IN ('work', 'merge', 'research', 'plan'));
ALTER TABLE runs ADD COLUMN triggered_by TEXT CHECK (triggered_by IS NULL OR triggered_by IN ('dispatcher', 'manual'));
ALTER TABLE runs ADD COLUMN subject_key TEXT CHECK (subject_key IS NULL OR length(subject_key) BETWEEN 1 AND 100);
ALTER TABLE runs ADD COLUMN profile_id BLOB REFERENCES dispatch_profiles (id) ON DELETE SET NULL;
ALTER TABLE runs ADD COLUMN profile_name TEXT;
ALTER TABLE runs ADD COLUMN model TEXT;
ALTER TABLE runs ADD COLUMN effort TEXT;
ALTER TABLE runs ADD COLUMN max_turns INTEGER CHECK (max_turns IS NULL OR max_turns > 0);
ALTER TABLE runs ADD COLUMN timeout_minutes INTEGER CHECK (timeout_minutes IS NULL OR timeout_minutes > 0);
ALTER TABLE runs ADD COLUMN account_id BLOB REFERENCES claude_accounts (id) ON DELETE SET NULL;
ALTER TABLE runs ADD COLUMN config_hash TEXT REFERENCES dispatch_config_versions (hash);
ALTER TABLE runs ADD COLUMN cwd TEXT;
ALTER TABLE runs ADD COLUMN command_args TEXT CHECK (command_args IS NULL OR json_valid(command_args));
ALTER TABLE runs ADD COLUMN prompt TEXT;
ALTER TABLE runs ADD COLUMN claude_session_id TEXT;
ALTER TABLE runs ADD COLUMN outcome TEXT CHECK (outcome IS NULL OR outcome IN (
  'done', 'review', 'human', 'abort', 'abort-final', 'blocked', 'question', 'pause-limit',
  'pause-claude-missing', 'setup-failed', 'taken', 'gone', 'other', 'merge-session', 'research', 'planned'));
ALTER TABLE runs ADD COLUMN abort_category TEXT CHECK (abort_category IS NULL OR abort_category IN (
  'timeout', 'max-turns', 'context', 'exit-code', 'claude-error', 'usage-limit', 'start-failed', 'vanished'));
ALTER TABLE runs ADD COLUMN abort_reason TEXT;
ALTER TABLE runs ADD COLUMN num_turns INTEGER;
ALTER TABLE runs ADD COLUMN cost_usd REAL;
ALTER TABLE runs ADD COLUMN input_tokens INTEGER;
ALTER TABLE runs ADD COLUMN output_tokens INTEGER;
ALTER TABLE runs ADD COLUMN cache_read_tokens INTEGER;
ALTER TABLE runs ADD COLUMN cache_write_tokens INTEGER;
ALTER TABLE runs ADD COLUMN model_usage TEXT CHECK (model_usage IS NULL OR json_valid(model_usage));
ALTER TABLE runs ADD COLUMN denied_count INTEGER;

CREATE INDEX ix_runs_subject ON runs (subject_key, created_at DESC) WHERE subject_key IS NOT NULL;
CREATE INDEX ix_runs_config ON runs (config_hash) WHERE config_hash IS NOT NULL;
CREATE INDEX ix_runs_claude_session ON runs (claude_session_id) WHERE claude_session_id IS NOT NULL;

CREATE TABLE run_denied_tools (
  run_id BLOB NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
  tool   TEXT NOT NULL CHECK (length(tool) BETWEEN 1 AND 200),
  count  INTEGER NOT NULL DEFAULT 1 CHECK (count >= 1),
  PRIMARY KEY (run_id, tool)
) STRICT;

-- Files a run left on its runner; the runner is the local one until runs can go elsewhere
CREATE TABLE run_artifacts (
  id          BLOB PRIMARY KEY CHECK (length(id) = 16),
  run_id      BLOB NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK (kind IN ('screenshot')),
  file        TEXT NOT NULL CHECK (length(file) BETWEEN 1 AND 200),
  media_type  TEXT NOT NULL,
  bytes       INTEGER NOT NULL CHECK (bytes >= 0),
  tool_use_id TEXT,
  created_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_run_artifacts_run ON run_artifacts (run_id, kind, created_at);
