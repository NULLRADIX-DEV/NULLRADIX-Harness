-- no-transaction
-- rebuilds runs (sqlite.org/lang_altertable.html) with foreign keys off so no referencing row changes
PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

CREATE TABLE runs_new (
  id                BLOB PRIMARY KEY CHECK (length(id) = 16),
  label             TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 200),
  status            TEXT NOT NULL
                    CHECK (status IN ('starting', 'running', 'succeeded', 'failed', 'cancelled', 'timed-out', 'lost')),
  pid               INTEGER CHECK (pid > 0),
  exit_code         INTEGER,
  error             TEXT,
  created_at        INTEGER NOT NULL,
  started_at        INTEGER,
  finished_at       INTEGER,
  version           INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  kind              TEXT CHECK (kind IS NULL OR kind IN ('work', 'merge', 'research', 'plan')),
  triggered_by      TEXT CHECK (triggered_by IS NULL OR triggered_by IN ('dispatcher', 'manual')),
  subject_key       TEXT CHECK (subject_key IS NULL OR length(subject_key) BETWEEN 1 AND 100),
  profile_id        BLOB REFERENCES dispatch_profiles (id) ON DELETE SET NULL,
  profile_name      TEXT,
  model             TEXT,
  effort            TEXT,
  max_turns         INTEGER CHECK (max_turns IS NULL OR max_turns > 0),
  timeout_minutes   INTEGER CHECK (timeout_minutes IS NULL OR timeout_minutes > 0),
  account_id        BLOB REFERENCES claude_accounts (id) ON DELETE SET NULL,
  config_hash       TEXT REFERENCES dispatch_config_versions (hash),
  cwd               TEXT,
  command_args      TEXT CHECK (command_args IS NULL OR json_valid(command_args)),
  prompt            TEXT,
  claude_session_id TEXT,
  outcome           TEXT CHECK (outcome IS NULL OR outcome IN (
    'done', 'review', 'human', 'abort', 'abort-final', 'blocked', 'question', 'pause-limit',
    'pause-claude-missing', 'setup-failed', 'taken', 'gone', 'other', 'merge-session', 'research', 'planned',
    'start-failed')),
  abort_category    TEXT CHECK (abort_category IS NULL OR abort_category IN (
    'timeout', 'max-turns', 'context', 'exit-code', 'claude-error', 'usage-limit', 'start-failed', 'vanished')),
  abort_reason      TEXT,
  num_turns         INTEGER,
  cost_usd          REAL,
  input_tokens      INTEGER,
  output_tokens     INTEGER,
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  model_usage       TEXT CHECK (model_usage IS NULL OR json_valid(model_usage)),
  denied_count      INTEGER,
  agent             TEXT
) STRICT;

INSERT INTO runs_new (
  id, label, status, pid, exit_code, error, created_at, started_at, finished_at, version,
  kind, triggered_by, subject_key, profile_id, profile_name, model, effort, max_turns, timeout_minutes,
  account_id, config_hash, cwd, command_args, prompt, claude_session_id, outcome, abort_category,
  abort_reason, num_turns, cost_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
  model_usage, denied_count, agent)
SELECT
  id, label, status, pid, exit_code, error, created_at, started_at, finished_at, version,
  kind, triggered_by, subject_key, profile_id, profile_name, model, effort, max_turns, timeout_minutes,
  account_id, config_hash, cwd, command_args, prompt, claude_session_id, outcome, abort_category,
  abort_reason, num_turns, cost_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
  model_usage, denied_count, agent
FROM runs;

DROP TABLE runs;

ALTER TABLE runs_new RENAME TO runs;

CREATE INDEX ix_runs_created_at ON runs (created_at DESC);
CREATE INDEX ix_runs_active ON runs (status) WHERE status IN ('starting', 'running');
CREATE INDEX ix_runs_subject ON runs (subject_key, created_at DESC) WHERE subject_key IS NOT NULL;
CREATE INDEX ix_runs_config ON runs (config_hash) WHERE config_hash IS NOT NULL;
CREATE INDEX ix_runs_claude_session ON runs (claude_session_id) WHERE claude_session_id IS NOT NULL;

-- overflows and aborts when a reference dangles
SELECT abs(-9223372036854775807 - 1) FROM pragma_foreign_key_check LIMIT 1;

COMMIT;

PRAGMA foreign_keys = ON;
